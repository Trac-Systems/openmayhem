const parseSemverMajor = (value) => {
  const match = /^v?(\d+)\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.exec(
    String(value ?? "").trim()
  );
  return match ? Number(match[1]) : null;
};

const parseJsonMajor = (value) => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;

  for (const key of ["semver", "SemVer", "version", "pear"]) {
    const major = parseSemverMajor(value[key]);
    if (major !== null) return major;
  }

  return null;
};

export const parsePearMajor = (output) => {
  const text = String(output ?? "").trim();
  if (!text) return null;

  try {
    const major = parseJsonMajor(JSON.parse(text));
    if (major !== null) return major;
  } catch (_e) {}

  const jsonStart = text.indexOf("{");
  const jsonEnd = text.lastIndexOf("}");
  if (jsonStart !== -1 && jsonEnd > jsonStart) {
    try {
      const major = parseJsonMajor(JSON.parse(text.slice(jsonStart, jsonEnd + 1)));
      if (major !== null) return major;
    } catch (_e) {}
  }

  const labelled = /(?:^|\s)SemVer\s*[:=]\s*v?(\d+)\.\d+\.\d+(?=$|\s)/im.exec(text);
  if (labelled) return Number(labelled[1]);

  const versions = [
    ...text.matchAll(/(?:^|[\s/])v?(\d+)\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?=$|\s)/g),
  ];
  return versions.length > 0 ? Number(versions[versions.length - 1][1]) : null;
};

export const selectPearRunnerMode = (output) => {
  const major = parsePearMajor(output);
  return major !== null && major < 3 ? "legacy" : "module";
};
