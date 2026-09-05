#!/usr/bin/env python3
"""Offline, pinned vLLM wheel rewrite; never import/build/install wheel code.

ZIP_STORED, sorted members, fixed timestamps and canonical permissions make
outputs reproducible without depending on a particular zlib version. Native
members are streamed, not buffered. The adjacent JSON is deterministic too.

--source-checkout needs only vllm/v1/attention/backends/flashinfer.py plus either:
  * .git with HEAD at FIX_COMMIT, its pinned parent, the backend's tree objects,
    and a clean backend index entry (other worktree files may be absent); or
  * vllm-specmeta-source.json containing exactly base_commit, fix_commit and
    backend_sha256, with the pinned values below. No Git state is needed for
    this export form. The manifest is an identity declaration, not a signature;
    the backend bytes are always independently checked against the built-in SHA.
No tests, _version.py, setup.py, dependencies or build artifacts are needed in
the source tree. _version.py is read only from the pinned upstream wheel.
"""

import argparse
import ast
import base64
import csv
from email import policy
from email.parser import BytesParser
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import zipfile


BASE_COMMIT = "ee0da84ab9e04ac7610e28580af62c365e898389"
FIX_COMMIT = "9a9cbb67ac23d78cf7384650b561ce7b122f770b"
OLD_BACKEND_SHA = "ce9071eade3d1a9c7dc9dd9f187212599e1c20058ffda901d6950b9dcde19913"
NEW_BACKEND_SHA = "01678024fc88e5f031b866fd6affbb0d82917c271938d8c5878fa0e68c023ab5"
UPSTREAM_HASHES = {
    "vllm-0.24.0-cp38-abi3-manylinux_2_28_aarch64.whl":
        "700db71c3cf14697d42583521f38b12fac38db1e7a8ad062e8e4d63a5dadebd5",
    "vllm-0.24.0-cp38-abi3-manylinux_2_28_x86_64.whl":
        "2d2831aeba311292250df0132dbc4d8e9f42c654536eaec48e6fe58acb1822cf",
}
UPSTREAM_WHEEL_TAGS = {
    "vllm-0.24.0-cp38-abi3-manylinux_2_28_aarch64.whl":
        ["cp38-abi3-linux_aarch64"],
    "vllm-0.24.0-cp38-abi3-manylinux_2_28_x86_64.whl":
        ["cp38-abi3-linux_x86_64"],
}
VERSION = "0.24.0+mayhem.specmeta1"
BACKEND = "vllm/v1/attention/backends/flashinfer.py"
VERSION_FILE = "vllm/_version.py"
DIST = "vllm-0.24.0.dist-info"
NEW_DIST = "vllm-" + VERSION + ".dist-info"
RECORD = DIST + "/RECORD"
CHUNK = 1024 * 1024
MAX_ARCHIVE = 16 * 1024**3
MAX_MEMBER = 8 * 1024**3
MAX_TOTAL = 32 * 1024**3
MAX_SMALL = 16 * 1024**2
MAX_CENTRAL = 32 * 1024**2
MAX_ENTRIES = 50000


class InvalidWheel(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise InvalidWheel(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def digest_stream(stream, sink=None, limit=MAX_ARCHIVE):
    digest, size = hashlib.sha256(), 0
    while True:
        chunk = stream.read(CHUNK)
        if not chunk:
            break
        size += len(chunk)
        require(size <= limit, "stream exceeds size bound")
        digest.update(chunk)
        if sink is not None:
            sink.write(chunk)
    return digest.hexdigest(), size


def safe_path(name):
    require(isinstance(name, str) and 0 < len(name) <= 4096,
            "invalid member path length")
    require(not any(ord(c) < 32 or ord(c) == 127 or c in "\\:" for c in name),
            "unsafe member path: " + repr(name))
    parts = name.rstrip("/").split("/")
    require(all(p and p not in (".", "..") and not p.endswith((".", " "))
                for p in parts), "unsafe member path: " + repr(name))
    require(not name.endswith("//"), "unsafe directory path")


def git(checkout, *args):
    # These are read-only plumbing commands, never diff/filter/build commands.
    env = dict(os.environ, GIT_NO_REPLACE_OBJECTS="1", GIT_OPTIONAL_LOCKS="0")
    for key in list(env):
        if key.startswith("GIT_") and key not in (
                "GIT_NO_REPLACE_OBJECTS", "GIT_OPTIONAL_LOCKS"):
            del env[key]
    result = subprocess.run(
        ["git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-C", str(checkout), *args], env=env, check=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
    require(len(result.stdout) <= MAX_CENTRAL, "source listing too large")
    return result.stdout


def fixed_source(checkout):
    checkout = Path(checkout).resolve(strict=True)
    path = checkout / BACKEND
    require(all(not parent.is_symlink() for parent in
                (path, *path.parents) if parent != checkout), "source symlink rejected")
    require(stat.S_ISREG(path.stat().st_mode), "backend must be a regular file")
    with path.open("rb") as stream:
        backend = stream.read(MAX_SMALL + 1)
    require(len(backend) <= MAX_SMALL and sha256(backend) == NEW_BACKEND_SHA,
            "fixed backend SHA256 mismatch")
    if os.path.lexists(checkout / ".git"):
        require(git(checkout, "rev-parse", "HEAD").strip().decode() == FIX_COMMIT,
                "source HEAD is not the pinned fix commit")
        require(git(checkout, "rev-parse", "HEAD^").strip().decode() == BASE_COMMIT,
                "source parent is not the pinned base commit")
        blob = hashlib.sha1(b"blob " + str(len(backend)).encode() + b"\0" + backend).hexdigest().encode()
        tree = git(checkout, "ls-tree", "--full-tree", "-z", "HEAD", "--", BACKEND)
        # Git owns the tracked mode; transported source bytes need not be executable.
        require(tree in {mode + b" blob " + blob + b"\t" + BACKEND.encode() + b"\0"
                         for mode in (b"100644", b"100755")},
                "backend does not match pinned source tree")
        mode = tree.split(b" ", 1)[0]
        require(git(checkout, "ls-files", "--stage", "-z", "--", BACKEND) ==
                mode + b" " + blob + b" 0\t" + BACKEND.encode() + b"\0",
                "source backend index is not clean")
    else:
        manifest = checkout / "vllm-specmeta-source.json"
        require(not manifest.is_symlink() and stat.S_ISREG(manifest.stat().st_mode),
                "source manifest must be regular")
        with manifest.open("rb") as stream:
            data = stream.read(MAX_SMALL + 1)
        require(len(data) <= MAX_SMALL, "source manifest too large")
        require(json.loads(data) == {"base_commit": BASE_COMMIT,
                                   "fix_commit": FIX_COMMIT,
                                   "backend_sha256": NEW_BACKEND_SHA},
                "exported source identity mismatch")
    return backend


def check_zip_bounds(stream):
    require(os.fstat(stream.fileno()).st_size <= MAX_ARCHIVE, "archive too large")
    # zipfile's bounded EOCD/ZIP64 parser avoids loading a huge central directory.
    end = zipfile._EndRecData(stream)
    require(end is not None, "missing ZIP end record")
    require(end[zipfile._ECD_SIZE] <= MAX_CENTRAL, "central directory too large")
    require(end[zipfile._ECD_ENTRIES_TOTAL] <= MAX_ENTRIES, "too many ZIP entries")
    require(end[zipfile._ECD_DISK_NUMBER] == 0 and
            end[zipfile._ECD_DISK_START] == 0 and
            end[zipfile._ECD_ENTRIES_THIS_DISK] == end[zipfile._ECD_ENTRIES_TOTAL],
            "multidisk ZIP unsupported")
    stream.seek(0)


def inventory(wheel):
    infos, aliases, total = {}, set(), 0
    require(len(wheel.infolist()) <= MAX_ENTRIES, "too many ZIP entries")
    for info in wheel.infolist():
        name = info.filename
        require(info.orig_filename == name, "truncated ZIP path")
        safe_path(name)
        alias = name.rstrip("/").casefold()
        require(alias not in aliases, "duplicate ZIP path: " + name)
        aliases.add(alias)
        mode = stat.S_IFMT(info.external_attr >> 16)
        require(mode in (0, stat.S_IFDIR if info.is_dir() else stat.S_IFREG),
                "symlink or special ZIP member: " + name)
        require(not info.flag_bits & 1 and info.compress_type in
                (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED),
                "encrypted or unsupported ZIP member")
        require(0 <= info.file_size <= MAX_MEMBER and
                0 <= info.compress_size <= MAX_ARCHIVE, "ZIP member too large")
        require(not info.is_dir() or info.file_size == 0, "nonempty ZIP directory")
        total += info.file_size
        require(total <= MAX_TOTAL, "expanded ZIP too large")
        for part in name.rstrip("/").split("/"):
            require(not part.endswith(".dist-info") or part == DIST,
                    "unknown dist-info directory")
        require(name not in (DIST + "/RECORD.jws", DIST + "/RECORD.p7s"),
                "signed RECORD cannot be rewritten")
        infos[name] = info
    files = {n.rstrip("/").casefold() for n, i in infos.items() if not i.is_dir()}
    for name in infos:
        parts = name.rstrip("/").casefold().split("/")
        require(all("/".join(parts[:i]) not in files for i in range(1, len(parts))),
                "ZIP file/directory collision")
    require({BACKEND, VERSION_FILE, RECORD, DIST + "/METADATA", DIST + "/WHEEL"}
            <= infos.keys(), "required wheel member missing")
    return infos


def small(wheel, name):
    require(wheel.getinfo(name).file_size <= MAX_SMALL, "metadata member too large")
    with wheel.open(name) as stream:
        data = stream.read(MAX_SMALL + 1)
    require(len(data) <= MAX_SMALL, "metadata exceeds size bound")
    return data


def read_record(data, infos):
    rows = {}
    for row in csv.reader(io.StringIO(data.decode("utf-8"), newline=""), strict=True):
        require(len(row) == 3, "invalid RECORD row")
        name, digest, size = row
        safe_path(name)
        require(name not in rows and name in infos and not infos[name].is_dir(),
                "duplicate or unknown RECORD path")
        if name == RECORD:
            require(digest == size == "", "RECORD must not hash itself")
        else:
            require(digest.startswith("sha256=") and len(digest) == 50,
                    "RECORD requires SHA256")
            encoded = digest[7:]
            raw = base64.b64decode(encoded + "=", altchars=b"-_", validate=True)
            require(base64.urlsafe_b64encode(raw).decode().rstrip("=") == encoded,
                    "noncanonical RECORD hash")
            require(size == str(infos[name].file_size), "RECORD size mismatch")
        rows[name] = (digest, size)
    require(rows.keys() == {n for n, i in infos.items() if not i.is_dir()},
            "RECORD does not cover every file")
    return rows


def record_hash(hex_digest):
    return "sha256=" + base64.urlsafe_b64encode(bytes.fromhex(hex_digest)).decode().rstrip("=")


def verify_record(name, digest, size, rows):
    if name != RECORD:
        require(rows[name] == (record_hash(digest), str(size)),
                "RECORD hash/size mismatch: " + name)


def rewrite_version(data):
    tree = ast.parse(data)
    fields = {
        ("__version__", "version"): ("0.24.0", VERSION),
        ("__version_tuple__", "version_tuple"):
            ((0, 24, 0), (0, 24, 0, "mayhem.specmeta1")),
        ("__commit_id__", "commit_id"): ("gee0da84ab", "g" + FIX_COMMIT[:9]),
    }
    names = {name for pair in fields for name in pair}
    lines = data.splitlines(keepends=True)
    offsets = [0]
    for line in lines:
        offsets.append(offsets[-1] + len(line))
    edits, seen = [], set()
    for node in tree.body:
        if isinstance(node, ast.Assign):
            require(all(isinstance(t, ast.Name) for t in node.targets),
                    "unknown version assignment")
            keys = tuple(t.id for t in node.targets)
            if keys == ("__all__",):
                require(set(ast.literal_eval(node.value)) == names,
                        "unknown version exports")
                continue
            require(keys in fields and keys not in seen, "unknown version fields")
            old, new = fields[keys]
            require(ast.literal_eval(node.value) == old, "unknown original version")
            seen.add(keys)
            value = node.value
            start = offsets[value.lineno - 1] + value.col_offset
            end = offsets[value.end_lineno - 1] + value.end_col_offset
            edits.append((start, end, repr(new).encode("ascii")))
        elif isinstance(node, ast.AnnAssign):
            require(isinstance(node.target, ast.Name) and node.target.id in names
                    and node.value is None, "unknown version annotation")
        elif isinstance(node, ast.ImportFrom):
            require(node.level == 0 and node.module in ("__future__", "typing")
                    and all(n.asname is None and n.name in
                            ("annotations", "Tuple", "Union") for n in node.names),
                    "unknown version import")
        else:
            require(isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant)
                    and isinstance(node.value.value, str), "unknown version schema")
    require(seen == fields.keys(), "missing version fields")
    for start, end, replacement in sorted(edits, reverse=True):
        data = data[:start] + replacement + data[end:]
    return data


def headers(data):
    message = BytesParser(policy=policy.compat32).parsebytes(data)
    require(not message.defects, "malformed wheel metadata")
    return message


def rewrite_metadata(data):
    message = headers(data)
    require(message.get_all("Name") == ["vllm"] and
            message.get_all("Version") == ["0.24.0"], "unknown original METADATA")
    # Parse structurally, but splice only this header to preserve dependencies,
    # license fields and the description byte-for-byte (email reserializes them).
    lines = data.splitlines(keepends=True)
    for i, line in enumerate(lines):
        if line in (b"\n", b"\r\n"):
            break
        if line.lower().startswith(b"version:"):
            ending = b"\r\n" if line.endswith(b"\r\n") else b"\n"
            require(line.rstrip(b"\r\n").split(b":", 1)[1].strip() == b"0.24.0"
                    and (i + 1 == len(lines) or not lines[i + 1].startswith((b" ", b"\t"))),
                    "unknown Version header encoding")
            lines[i] = line.split(b":", 1)[0] + b": " + VERSION.encode() + ending
            return b"".join(lines)
    raise InvalidWheel("missing Version header")


def renamed(name):
    return NEW_DIST + name[len(DIST):] if name == DIST + "/" or name.startswith(DIST + "/") else name


def write_wheel(source, target, infos, rows, replacements):
    records, changes = [], []
    with zipfile.ZipFile(target, "w", compression=zipfile.ZIP_STORED,
                         allowZip64=True) as output:
        for name in sorted(infos, key=renamed):
            if name == RECORD:
                continue
            original = infos[name]
            new_name = renamed(name)
            info = zipfile.ZipInfo(new_name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            mode = 0o755 if original.is_dir() or original.external_attr >> 16 & 0o111 else 0o644
            info.external_attr = ((stat.S_IFDIR if original.is_dir() else stat.S_IFREG) | mode) << 16
            if original.is_dir():
                info.external_attr |= 0x10
            info.file_size = len(replacements[name]) if name in replacements else original.file_size
            with output.open(info, "w", force_zip64=info.file_size >= 2**31) as sink:
                if name in replacements:
                    data = replacements[name]
                    sink.write(data)
                    digest, size = sha256(data), len(data)
                    old_digest = sha256(small(source, name))
                else:
                    with source.open(original) as stream:
                        digest, size = digest_stream(stream, sink, original.file_size)
                    old_digest = digest
                    if not original.is_dir():
                        verify_record(name, digest, size, rows)
            if not original.is_dir():
                records.append((new_name, record_hash(digest), str(size)))
            if name != new_name or name in replacements:
                changes.append({"original": name, "output": new_name,
                                "original_sha256": old_digest, "sha256": digest})
        record_text = io.StringIO(newline="")
        writer = csv.writer(record_text, lineterminator="\n")
        writer.writerows(sorted(records + [(renamed(RECORD), "", "")]))
        record_data = record_text.getvalue().encode("utf-8")
        info = zipfile.ZipInfo(renamed(RECORD), (1980, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = (stat.S_IFREG | 0o644) << 16
        output.writestr(info, record_data)
        changes.append({"original": RECORD, "output": renamed(RECORD),
                        "original_sha256": sha256(small(source, RECORD)),
                        "sha256": sha256(record_data)})
    return sorted(changes, key=lambda row: row["output"])


def _package(wheel_path, output_dir, backend, expected_input_sha):
    """Internal fixture seam; the public entry point always selects pinned hashes."""
    wheel_path, output_dir = Path(wheel_path), Path(output_dir)
    require(wheel_path.name in UPSTREAM_HASHES, "unapproved upstream wheel filename")
    require(sha256(backend) == NEW_BACKEND_SHA, "fixed backend SHA256 mismatch")
    output_name = wheel_path.name.replace("vllm-0.24.0-", "vllm-" + VERSION + "-", 1)
    destinations = [output_dir / output_name, output_dir / (output_name + ".provenance.json")]
    owned, success = {}, False

    def remember(path):
        info = path.lstat()
        owned[path] = (info.st_dev, info.st_ino)

    try:
        with wheel_path.open("rb") as stream:
            require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), "wheel must be regular")
            check_zip_bounds(stream)
            input_sha, input_size = digest_stream(stream)
            require(input_sha == expected_input_sha, "upstream wheel SHA256 mismatch")
            stream.seek(0)
            with zipfile.ZipFile(stream) as source:
                infos = inventory(source)
                rows = read_record(small(source, RECORD), infos)
                originals = {n: small(source, n) for n in
                             (BACKEND, VERSION_FILE, DIST + "/METADATA", DIST + "/WHEEL")}
                for name, data in originals.items():
                    verify_record(name, sha256(data), len(data), rows)
                require(sha256(originals[BACKEND]) == OLD_BACKEND_SHA,
                        "unknown original backend SHA256")
                wheel_headers = headers(originals[DIST + "/WHEEL"])
                # Pinned upstream metadata may differ from the published filename.
                require(wheel_headers.get_all("Tag") == UPSTREAM_WHEEL_TAGS[wheel_path.name] and
                        wheel_headers.get_all("Wheel-Version") == ["1.0"] and
                        wheel_headers.get_all("Root-Is-Purelib") == ["false"],
                        "unknown original WHEEL metadata")
                replacements = {BACKEND: backend,
                                VERSION_FILE: rewrite_version(originals[VERSION_FILE]),
                                DIST + "/METADATA": rewrite_metadata(originals[DIST + "/METADATA"])}
                output_dir.mkdir(parents=True, exist_ok=True)
                require(not any(os.path.lexists(p) for p in destinations), "output already exists")
                with tempfile.NamedTemporaryFile(prefix=".vllm-", suffix=".tmp",
                                                 dir=output_dir, delete=False) as target:
                    temporary = Path(target.name)
                    remember(temporary)
                    changes = write_wheel(source, target, infos, rows, replacements)
                    target.flush()
                    os.fsync(target.fileno())
                # Detect an input changed during streaming, including ZIP headers.
                stream.seek(0)
                require(digest_stream(stream) == (input_sha, input_size),
                        "upstream wheel changed during packaging")
        with temporary.open("rb") as stream:
            output_sha, output_size = digest_stream(stream, limit=MAX_TOTAL + MAX_CENTRAL)
        provenance = {
            "schema_version": 1, "upstream_version": "0.24.0", "version": VERSION,
            "base_commit": BASE_COMMIT, "fix_commit": FIX_COMMIT,
            "backend": {"path": BACKEND, "original_sha256": OLD_BACKEND_SHA,
                        "sha256": NEW_BACKEND_SHA},
            "upstream_wheel": {"filename": wheel_path.name, "sha256": input_sha, "size": input_size},
            "output_wheel": {"filename": output_name, "sha256": output_sha, "size": output_size},
            "changed_members": changes,
        }
        with tempfile.NamedTemporaryFile(prefix=".vllm-", suffix=".tmp",
                                         dir=output_dir, delete=False) as target:
            sidecar = Path(target.name)
            remember(sidecar)
            target.write((json.dumps(provenance, sort_keys=True, indent=2) + "\n").encode())
            target.flush()
            os.fsync(target.fileno())
        for temporary_path, destination in zip((temporary, sidecar), destinations):
            os.link(temporary_path, destination)  # Atomic publication, never overwrite.
            owned[destination] = owned[temporary_path]
        success = True
        return tuple(destinations)
    finally:
        for path, identity in owned.items():
            if success and path in destinations:
                continue
            try:
                info = path.lstat()
                if (info.st_dev, info.st_ino) == identity:
                    path.unlink()
            except FileNotFoundError:
                pass


def package(upstream_wheel, source_checkout, output_dir):
    name = Path(upstream_wheel).name
    require(name in UPSTREAM_HASHES, "unapproved upstream wheel filename")
    return _package(upstream_wheel, output_dir, fixed_source(source_checkout),
                    UPSTREAM_HASHES[name])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-wheel", required=True, type=Path)
    parser.add_argument("--source-checkout", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        for path in package(args.upstream_wheel, args.source_checkout, args.output_dir):
            print(path)
    except (ValueError, OSError, SyntaxError, csv.Error, zipfile.BadZipFile,
            RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, "packaging failed: " + str(error) + "\n")


if __name__ == "__main__":
    main()
