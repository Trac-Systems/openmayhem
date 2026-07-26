import contextlib
import hashlib
import importlib.util
import json
import math
import os
import pathlib
import re
import sys
import time
import types


API_VERSION = 1
SOURCE_COMMIT = "ffd0d081401257fee31150d30c494b2f98910fc0"
MODEL_REVISION = "5f89b4307696d669c3df1d38ae057e6e1728b107"
MAX_ENCODER_TOKENS = 1024
MAX_DECODER_TOKENS = 512
MAX_PROTOCOL_LINE_BYTES = 2 * 1024 * 1024
MAX_PROMPT_BYTES = 1024 * 1024
MAX_TOOLS = 10
MAX_SCHEMA_DEPTH = 32
MAX_SCHEMA_NODES = 512
MAX_SCHEMA_BRANCHES = 32
MAX_SCHEMA_VALIDATION_WORK = 4096
SUPPORTED_SCHEMA_TYPES = frozenset(
    ("array", "boolean", "integer", "null", "number", "object", "string")
)
SUPPORTED_SCHEMA_KEYWORDS = frozenset(
    (
        "$defs",
        "$ref",
        "additionalProperties",
        "allOf",
        "anyOf",
        "const",
        "default",
        "definitions",
        "deprecated",
        "description",
        "enum",
        "examples",
        "exclusiveMaximum",
        "exclusiveMinimum",
        "items",
        "maxItems",
        "maxLength",
        "maxProperties",
        "maximum",
        "minItems",
        "minLength",
        "minProperties",
        "minimum",
        "multipleOf",
        "nullable",
        "oneOf",
        "properties",
        "readOnly",
        "required",
        "title",
        "type",
        "uniqueItems",
        "writeOnly",
    )
)
REQUIRED_MODEL_FILES = (
    "config.json",
    "configuration_needle.py",
    "model.safetensors",
    "modeling_needle.py",
    "special_tokens_map.json",
    "tokenization_needle.py",
    "tokenizer.model",
    "tokenizer_config.json",
)

_runtime = None


class ProtocolError(Exception):
    pass


class _SchemaWorkLimit(ProtocolError):
    pass


class _SchemaWorkBudget:
    def __init__(self, limit, label):
        self.limit = limit
        self.label = label
        self.used = 0

    def spend(self):
        self.used += 1
        if self.used > self.limit:
            raise _SchemaWorkLimit(
                f"{self.label} exceeded its deterministic work budget of {self.limit}"
            )


def _reject_constant(value):
    raise ProtocolError(f"non-finite JSON number {value!r} is forbidden")


def _exact_object(value, fields, label):
    if not isinstance(value, dict):
        raise ProtocolError(f"{label} must be an object")
    actual = set(value)
    expected = set(fields)
    if actual != expected:
        raise ProtocolError(
            f"{label} fields must be exactly {sorted(expected)!r}, got {sorted(actual)!r}"
        )
    return value


def _integer(value, label, minimum=0, maximum=(1 << 63) - 1):
    if isinstance(value, bool) or not isinstance(value, int):
        raise ProtocolError(f"{label} must be an integer")
    if value < minimum or value > maximum:
        raise ProtocolError(f"{label} is outside its allowed range")
    return value


def _offline_environment():
    for name in (
        "HF_HUB_OFFLINE",
        "HF_DATASETS_OFFLINE",
        "TRANSFORMERS_OFFLINE",
        "PIP_NO_INDEX",
        "UV_OFFLINE",
    ):
        if os.environ.get(name) != "1":
            raise ProtocolError(f"{name} must be 1 for the offline Needle worker")


def _real_regular_file(path):
    return path.is_file() and not path.is_symlink()


def _sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _canonical_json(value):
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    )


def _json_equality_key(value, budget, depth=0):
    if depth > MAX_SCHEMA_DEPTH:
        raise ProtocolError("JSON value nesting exceeds its equality bound")
    budget.spend()
    if value is None:
        return ("null",)
    if isinstance(value, bool):
        return ("boolean", value)
    if _is_number(value):
        return ("number", value)
    if isinstance(value, str):
        return ("string", value)
    if isinstance(value, list):
        return (
            "array",
            tuple(
                _json_equality_key(item, budget, depth + 1)
                for item in value
            ),
        )
    if isinstance(value, dict):
        return (
            "object",
            tuple(
                (
                    name,
                    _json_equality_key(child, budget, depth + 1),
                )
                for name, child in sorted(value.items())
            ),
        )
    raise ProtocolError(f"unsupported JSON value {type(value).__name__!r}")


def _snake_case(name):
    value = re.sub(r"[^a-zA-Z0-9_]+", "_", name)
    value = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", value)
    value = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", value)
    return re.sub(r"_+", "_", value).lower().strip("_")


def _normalize_parameter_schema(parameters, label, schema_budget):
    if parameters is None:
        parameters = {"type": "object", "properties": {}}
    if not isinstance(parameters, dict):
        raise ProtocolError(f"{label} parameters must be a JSON Schema object")
    schema_type = parameters.get("type", "object")
    if schema_type != "object":
        raise ProtocolError(f"{label} parameters must have type object")
    _validate_schema_surface(parameters, label, budget=schema_budget)
    properties = parameters.get("properties", {})
    required = parameters.get("required", [])
    if not isinstance(properties, dict):
        raise ProtocolError(f"{label} properties must be an object")
    if (
        not isinstance(required, list)
        or any(not isinstance(name, str) for name in required)
        or len(set(required)) != len(required)
    ):
        raise ProtocolError(f"{label} required must contain unique strings")
    unknown_required = set(required) - set(properties)
    if unknown_required:
        raise ProtocolError(
            f"{label} requires unknown properties {sorted(unknown_required)!r}"
        )
    normalized = {}
    for name, schema in properties.items():
        if not isinstance(name, str) or not name:
            raise ProtocolError(f"{label} property names must be non-empty strings")
        if not isinstance(schema, dict):
            raise ProtocolError(f"{label} property {name!r} must be a schema object")
        model_type = schema.get("type", "string")
        if model_type == "integer":
            model_type = "number"
        description = schema.get("description", "")
        if not isinstance(description, str):
            raise ProtocolError(f"{label} property {name!r} description must be a string")
        enum = schema.get("enum")
        if isinstance(enum, list) and enum:
            allowed = ", ".join(_canonical_json(value) for value in enum)
            description = f"{description.rstrip('.')} Allowed values: {allowed}.".strip()
        normalized[name] = {
            "type": model_type,
            "description": description,
            "required": name in required,
        }
    return normalized, parameters


def _schema_nonnegative_integer(schema, keyword, label):
    value = schema[keyword]
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ProtocolError(f"{label} {keyword} must be a non-negative integer")
    return value


def _schema_finite_number(schema, keyword, label):
    value = schema[keyword]
    if not _is_number(value):
        raise ProtocolError(f"{label} {keyword} must be a finite number")
    return value


def _validate_schema_surface(
    schema,
    label,
    root=None,
    depth=0,
    budget=None,
    active=None,
    visited=None,
):
    if depth > MAX_SCHEMA_DEPTH:
        raise ProtocolError(f"{label} schema nesting exceeds its bound")
    if not isinstance(schema, dict):
        raise ProtocolError(f"{label} schema entries must be objects")
    root = schema if root is None else root
    budget = (
        _SchemaWorkBudget(MAX_SCHEMA_NODES, "JSON Schema admission")
        if budget is None
        else budget
    )
    active = set() if active is None else active
    visited = set() if visited is None else visited
    identity = id(schema)
    if identity in active:
        raise ProtocolError(f"{label} contains a recursive JSON Schema reference")
    if identity in visited:
        return
    budget.spend()
    unsupported = set(schema) - SUPPORTED_SCHEMA_KEYWORDS
    if unsupported:
        raise ProtocolError(
            f"{label} uses unsupported JSON Schema keywords {sorted(unsupported)!r}"
        )
    active.add(identity)

    expected = schema.get("type")
    if expected is not None:
        expected_types = expected if isinstance(expected, list) else [expected]
        if (
            not expected_types
            or any(not isinstance(item, str) for item in expected_types)
            or len(set(expected_types)) != len(expected_types)
            or any(item not in SUPPORTED_SCHEMA_TYPES for item in expected_types)
        ):
            raise ProtocolError(f"{label} type contains unsupported or duplicate values")
    if "nullable" in schema and not isinstance(schema["nullable"], bool):
        raise ProtocolError(f"{label} nullable must be a boolean")
    for keyword in ("title", "description"):
        if keyword in schema and not isinstance(schema[keyword], str):
            raise ProtocolError(f"{label} {keyword} must be a string")
    for keyword in ("deprecated", "readOnly", "writeOnly", "uniqueItems"):
        if keyword in schema and not isinstance(schema[keyword], bool):
            raise ProtocolError(f"{label} {keyword} must be a boolean")
    if "examples" in schema and not isinstance(schema["examples"], list):
        raise ProtocolError(f"{label} examples must be an array")
    for keyword in ("const", "default", "enum", "examples"):
        if keyword not in schema:
            continue
        try:
            _canonical_json(schema[keyword])
        except (TypeError, ValueError) as error:
            raise ProtocolError(f"{label} {keyword} must contain finite JSON") from error
    if "enum" in schema and (
        not isinstance(schema["enum"], list) or not schema["enum"]
    ):
        raise ProtocolError(f"{label} enum must be a non-empty array")
    if "enum" in schema:
        encoded = [
            _json_equality_key(value, budget)
            for value in schema["enum"]
        ]
        if len(encoded) != len(set(encoded)):
            raise ProtocolError(f"{label} enum values must be unique")
    required = schema.get("required")
    if required is not None and (
        not isinstance(required, list)
        or any(not isinstance(name, str) or not name for name in required)
        or len(set(required)) != len(required)
    ):
        raise ProtocolError(f"{label} required must contain unique non-empty strings")
    for keyword in (
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minProperties",
        "maxProperties",
    ):
        if keyword in schema:
            _schema_nonnegative_integer(schema, keyword, label)
    for lower, upper in (
        ("minItems", "maxItems"),
        ("minLength", "maxLength"),
        ("minProperties", "maxProperties"),
    ):
        if lower in schema and upper in schema and schema[lower] > schema[upper]:
            raise ProtocolError(f"{label} {lower} cannot exceed {upper}")
    for keyword in (
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
    ):
        if keyword in schema:
            _schema_finite_number(schema, keyword, label)
    if "multipleOf" in schema and schema["multipleOf"] <= 0:
        raise ProtocolError(f"{label} multipleOf must be positive")

    reference = schema.get("$ref")
    if reference is not None and (
        not isinstance(reference, str) or not reference.startswith("#/")
    ):
        raise ProtocolError(f"{label} uses a non-local JSON Schema reference")
    if reference is not None:
        target = _resolve_local_ref(root, reference)
        _validate_schema_surface(
            target,
            f"{label}.$ref({reference})",
            root,
            depth + 1,
            budget,
            active,
            visited,
        )
    for key in ("properties", "$defs", "definitions"):
        children = schema.get(key, {})
        if not isinstance(children, dict):
            raise ProtocolError(f"{label} {key} must be an object")
        for name, child in children.items():
            if not isinstance(name, str) or not name:
                raise ProtocolError(f"{label} {key} names must be non-empty strings")
            _validate_schema_surface(
                child,
                f"{label}.{key}.{name}",
                root,
                depth + 1,
                budget,
                active,
                visited,
            )
    if "items" in schema:
        child = schema["items"]
        if not isinstance(child, dict):
            raise ProtocolError(f"{label} items must be a schema object")
        _validate_schema_surface(
            child,
            f"{label}.items",
            root,
            depth + 1,
            budget,
            active,
            visited,
        )
    if "additionalProperties" in schema:
        child = schema["additionalProperties"]
        if not isinstance(child, (bool, dict)):
            raise ProtocolError(
                f"{label} additionalProperties must be a boolean or schema object"
            )
        if isinstance(child, dict):
            _validate_schema_surface(
                child,
                f"{label}.additionalProperties",
                root,
                depth + 1,
                budget,
                active,
                visited,
            )
    for key in ("allOf", "anyOf", "oneOf"):
        children = schema.get(key)
        if children is None:
            continue
        if not isinstance(children, list) or not children:
            raise ProtocolError(f"{label} {key} must be a non-empty array")
        if len(children) > MAX_SCHEMA_BRANCHES:
            raise ProtocolError(
                f"{label} {key} exceeds its {MAX_SCHEMA_BRANCHES}-branch bound"
            )
        for index, child in enumerate(children):
            _validate_schema_surface(
                child,
                f"{label}.{key}[{index}]",
                root,
                depth + 1,
                budget,
                active,
                visited,
            )
    active.remove(identity)
    visited.add(identity)


def _has_complex_parameter_schema(parameters):
    structural_keywords = {
        "$ref",
        "allOf",
        "anyOf",
        "items",
        "oneOf",
        "properties",
    }
    if {"$ref", "allOf", "anyOf", "oneOf"} & set(parameters):
        return True
    for schema in parameters.get("properties", {}).values():
        schema_type = schema.get("type")
        schema_types = schema_type if isinstance(schema_type, list) else [schema_type]
        if any(value in ("array", "object") for value in schema_types):
            return True
        if structural_keywords & set(schema):
            return True
    return False


def normalize_openai_tools(tools):
    if not isinstance(tools, list) or not tools:
        raise ProtocolError("Needle is tools-only and requires at least one tool")
    if len(tools) > MAX_TOOLS:
        raise ProtocolError(f"Needle accepts at most {MAX_TOOLS} tools")
    normalized_tools = []
    by_normalized_name = {}
    schema_budget = _SchemaWorkBudget(
        MAX_SCHEMA_NODES,
        "JSON Schema admission",
    )
    for index, tool in enumerate(tools):
        label = f"tool[{index}]"
        if not isinstance(tool, dict):
            raise ProtocolError(f"{label} must be an object")
        if tool.get("type") != "function":
            raise ProtocolError(f"{label} type must be function")
        function = tool.get("function")
        if not isinstance(function, dict):
            raise ProtocolError(f"{label}.function must be an object")
        name = function.get("name")
        if not isinstance(name, str) or not name or len(name) > 128:
            raise ProtocolError(f"{label} name must be 1 to 128 characters")
        normalized_name = _snake_case(name)
        if not normalized_name:
            raise ProtocolError(f"{label} name normalizes to an empty identifier")
        previous = by_normalized_name.get(normalized_name)
        if previous is not None:
            raise ProtocolError(
                f"tool names {previous!r} and {name!r} collide as {normalized_name!r}"
            )
        description = function.get("description", "")
        if description is None:
            description = ""
        if not isinstance(description, str):
            raise ProtocolError(f"{label} description must be a string")
        normalized_parameters, original_schema = _normalize_parameter_schema(
            function.get("parameters"),
            label,
            schema_budget,
        )
        by_normalized_name[normalized_name] = {
            "name": name,
            "schema": original_schema,
        }
        normalized_tool = {
            "description": description,
            "name": normalized_name,
            "parameters": normalized_parameters,
        }
        if _has_complex_parameter_schema(original_schema):
            normalized_tool["json_schema"] = original_schema
        normalized_tools.append(normalized_tool)
    return normalized_tools, by_normalized_name


def _token_ids(tokenizer, text):
    encoded = tokenizer.encode(text, add_special_tokens=False)
    if hasattr(encoded, "tolist"):
        encoded = encoded.tolist()
    if encoded and isinstance(encoded[0], list):
        if len(encoded) != 1:
            raise ProtocolError("Needle tokenizer returned an unexpected batch")
        encoded = encoded[0]
    if not isinstance(encoded, list) or any(
        isinstance(token, bool) or not isinstance(token, int) for token in encoded
    ):
        raise ProtocolError("Needle tokenizer returned invalid token IDs")
    return encoded


def build_encoder_tokens(tokenizer, prompt, normalized_tools):
    if not isinstance(prompt, str) or not prompt.strip():
        raise ProtocolError("Needle requires a non-empty single-shot query")
    if len(prompt.encode("utf-8")) > MAX_PROMPT_BYTES:
        raise ProtocolError("Needle query exceeds its byte bound")
    query_tokens = _token_ids(tokenizer, prompt)
    tools_json = _canonical_json(normalized_tools)
    tool_tokens = _token_ids(tokenizer, tools_json)
    combined = query_tokens + [int(tokenizer.tools_token_id)] + tool_tokens
    if len(combined) > MAX_ENCODER_TOKENS:
        raise ProtocolError(
            f"Needle query and tools require {len(combined)} encoder tokens, "
            f"exceeding {MAX_ENCODER_TOKENS}; input was not truncated"
        )
    return combined, tools_json


def _resolve_local_ref(root, reference):
    if not isinstance(reference, str) or not reference.startswith("#/"):
        raise ProtocolError(f"unsupported JSON Schema reference {reference!r}")
    value = root
    for raw in reference[2:].split("/"):
        part = raw.replace("~1", "/").replace("~0", "~")
        if not isinstance(value, dict) or part not in value:
            raise ProtocolError(f"unresolved JSON Schema reference {reference!r}")
        value = value[part]
    if not isinstance(value, dict):
        raise ProtocolError(f"JSON Schema reference {reference!r} is not an object")
    return value


def _schema_error(path, message):
    raise ProtocolError(f"tool arguments at {path}: {message}")


def validate_json_schema(
    value,
    schema,
    path="$",
    root=None,
    depth=0,
    budget=None,
    active_references=None,
):
    if depth > MAX_SCHEMA_DEPTH:
        _schema_error(path, "schema nesting exceeds its bound")
    if not isinstance(schema, dict):
        _schema_error(path, "schema must be an object")
    root = schema if root is None else root
    budget = (
        _SchemaWorkBudget(
            MAX_SCHEMA_VALIDATION_WORK,
            "JSON Schema validation",
        )
        if budget is None
        else budget
    )
    active_references = set() if active_references is None else active_references
    budget.spend()
    if value is None and schema.get("nullable") is True:
        return
    if "$ref" in schema:
        reference = schema["$ref"]
        if reference in active_references:
            _schema_error(path, f"recursive JSON Schema reference {reference!r}")
        active_references.add(reference)
        try:
            validate_json_schema(
                value,
                _resolve_local_ref(root, reference),
                path,
                root,
                depth + 1,
                budget,
                active_references,
            )
        finally:
            active_references.remove(reference)
    if "allOf" in schema:
        branches = schema["allOf"]
        if not isinstance(branches, list) or not branches:
            _schema_error(path, "allOf must be a non-empty array")
        for branch in branches:
            validate_json_schema(
                value,
                branch,
                path,
                root,
                depth + 1,
                budget,
                active_references,
            )
    if "anyOf" in schema:
        branches = schema["anyOf"]
        if not isinstance(branches, list) or not branches:
            _schema_error(path, "anyOf must be a non-empty array")
        matches = False
        for branch in branches:
            if _schema_matches(
                value,
                branch,
                path,
                root,
                depth + 1,
                budget,
                active_references,
            ):
                matches = True
                break
        if not matches:
            _schema_error(path, "does not match any allowed schema")
    if "oneOf" in schema:
        branches = schema["oneOf"]
        if not isinstance(branches, list) or not branches:
            _schema_error(path, "oneOf must be a non-empty array")
        matches = sum(
            _schema_matches(
                value,
                branch,
                path,
                root,
                depth + 1,
                budget,
                active_references,
            )
            for branch in branches
        )
        if matches != 1:
            _schema_error(path, "does not match exactly one allowed schema")
    if "const" in schema and not _json_equal(
        value,
        schema["const"],
        budget,
    ):
        _schema_error(path, "does not match const")
    if "enum" in schema:
        enum = schema["enum"]
        if not isinstance(enum, list) or not any(
            _json_equal(value, candidate, budget) for candidate in enum
        ):
            _schema_error(path, "is not one of the allowed enum values")

    expected = schema.get("type")
    if expected is not None:
        expected_types = expected if isinstance(expected, list) else [expected]
        if (
            not expected_types
            or any(not isinstance(item, str) for item in expected_types)
            or not any(_matches_type(value, item) for item in expected_types)
        ):
            _schema_error(path, f"does not match type {expected!r}")

    if isinstance(value, dict):
        required = schema.get("required", [])
        if not isinstance(required, list) or any(
            not isinstance(name, str) for name in required
        ):
            _schema_error(path, "schema required must be an array of strings")
        missing = [name for name in required if name not in value]
        if missing:
            _schema_error(path, f"is missing required properties {missing!r}")
        properties = schema.get("properties", {})
        if not isinstance(properties, dict):
            _schema_error(path, "schema properties must be an object")
        additional = schema.get("additionalProperties", True)
        for name, child in value.items():
            budget.spend()
            child_path = f"{path}.{name}"
            if name in properties:
                validate_json_schema(
                    child,
                    properties[name],
                    child_path,
                    root,
                    depth + 1,
                    budget,
                    active_references,
                )
            elif additional is False:
                _schema_error(child_path, "additional property is forbidden")
            elif isinstance(additional, dict):
                validate_json_schema(
                    child,
                    additional,
                    child_path,
                    root,
                    depth + 1,
                    budget,
                    active_references,
                )
        if "minProperties" in schema and len(value) < schema["minProperties"]:
            _schema_error(path, "has too few properties")
        if "maxProperties" in schema and len(value) > schema["maxProperties"]:
            _schema_error(path, "has too many properties")

    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            _schema_error(path, "has too few items")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            _schema_error(path, "has too many items")
        if schema.get("uniqueItems") is True:
            encoded = [
                _json_equality_key(item, budget)
                for item in value
            ]
            if len(encoded) != len(set(encoded)):
                _schema_error(path, "contains duplicate items")
        items = schema.get("items")
        if isinstance(items, dict):
            for index, child in enumerate(value):
                budget.spend()
                validate_json_schema(
                    child,
                    items,
                    f"{path}[{index}]",
                    root,
                    depth + 1,
                    budget,
                    active_references,
                )

    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            _schema_error(path, "is shorter than minLength")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            _schema_error(path, "is longer than maxLength")
    if _is_number(value):
        for keyword, comparison in (
            ("minimum", lambda left, right: left >= right),
            ("maximum", lambda left, right: left <= right),
            ("exclusiveMinimum", lambda left, right: left > right),
            ("exclusiveMaximum", lambda left, right: left < right),
        ):
            if keyword in schema and not comparison(value, schema[keyword]):
                _schema_error(path, f"violates {keyword}")
        if "multipleOf" in schema:
            divisor = schema["multipleOf"]
            if not _is_number(divisor) or divisor <= 0:
                _schema_error(path, "schema multipleOf must be positive")
            quotient = value / divisor
            if not math.isclose(quotient, round(quotient), rel_tol=1e-9, abs_tol=1e-9):
                _schema_error(path, "violates multipleOf")


def _schema_matches(
    value,
    schema,
    path,
    root,
    depth,
    budget,
    active_references,
):
    try:
        validate_json_schema(
            value,
            schema,
            path,
            root,
            depth,
            budget,
            active_references,
        )
        return True
    except _SchemaWorkLimit:
        raise
    except ProtocolError:
        return False


def _is_number(value):
    if isinstance(value, bool):
        return False
    if isinstance(value, int):
        return True
    return isinstance(value, float) and math.isfinite(value)


def _json_equal(left, right, budget):
    return _json_equality_key(left, budget) == _json_equality_key(right, budget)


def _matches_type(value, expected):
    if expected == "null":
        return value is None
    if expected == "boolean":
        return isinstance(value, bool)
    if expected == "integer":
        return _is_number(value) and (
            isinstance(value, int) or value.is_integer()
        )
    if expected == "number":
        return _is_number(value)
    if expected == "string":
        return isinstance(value, str)
    if expected == "array":
        return isinstance(value, list)
    if expected == "object":
        return isinstance(value, dict)
    raise ProtocolError(f"unsupported JSON Schema type {expected!r}")


def parse_and_validate_calls(text, tool_map):
    if not isinstance(text, str):
        raise ProtocolError("Needle output must be text")
    text = text.strip()
    if text.startswith("<tool_call>"):
        text = text[len("<tool_call>") :].lstrip()
    try:
        calls = json.loads(text, parse_constant=_reject_constant)
    except json.JSONDecodeError as error:
        raise ProtocolError(f"Needle emitted invalid tool-call JSON: {error}") from error
    if isinstance(calls, dict):
        calls = [calls]
    if not isinstance(calls, list) or not calls:
        raise ProtocolError("Needle must emit a non-empty array of tool calls")
    normalized = []
    validation_budget = _SchemaWorkBudget(
        MAX_SCHEMA_VALIDATION_WORK,
        "JSON Schema validation",
    )
    for index, call in enumerate(calls):
        validation_budget.spend()
        if not isinstance(call, dict) or set(call) != {"name", "arguments"}:
            raise ProtocolError(
                f"Needle tool call {index} must contain exactly name and arguments"
            )
        name = call["name"]
        if not isinstance(name, str) or name not in tool_map:
            raise ProtocolError(f"Needle emitted unknown tool name {name!r}")
        arguments = call["arguments"]
        if isinstance(arguments, str):
            try:
                arguments = json.loads(arguments, parse_constant=_reject_constant)
            except json.JSONDecodeError as error:
                raise ProtocolError(
                    f"Needle emitted invalid JSON arguments for {name!r}"
                ) from error
        if not isinstance(arguments, dict):
            raise ProtocolError(f"Needle arguments for {name!r} must be an object")
        binding = tool_map[name]
        try:
            validate_json_schema(
                arguments,
                binding["schema"],
                budget=validation_budget,
            )
        except ProtocolError as error:
            raise ProtocolError(
                f"Needle tool call {index} for {binding['name']!r} has invalid "
                f"argument keys {sorted(arguments)!r}: {error}"
            ) from error
        normalized.append({"arguments": arguments, "name": binding["name"]})
    return normalized


def first_complete_tool_call(text, tool_map):
    if not isinstance(text, str):
        raise ProtocolError("Needle output must be text")
    candidate = text.lstrip()
    if not candidate:
        return None
    tag = "<tool_call>"
    if tag.startswith(candidate):
        return None
    if candidate.startswith(tag):
        candidate = candidate[len(tag) :].lstrip()
    if not candidate:
        return None
    if candidate.startswith("["):
        candidate = candidate[1:].lstrip()
    if not candidate or not candidate.startswith("{"):
        return None
    try:
        value, _ = json.JSONDecoder(
            parse_constant=_reject_constant,
        ).raw_decode(candidate)
    except json.JSONDecodeError:
        return None
    if not isinstance(value, dict):
        raise ProtocolError("Needle first tool call must be an object")
    calls = parse_and_validate_calls(_canonical_json(value), tool_map)
    if len(calls) != 1:
        raise ProtocolError("Needle non-parallel generation must produce one tool call")
    return calls[0]


def _load_local_module(package_name, module_name, path):
    qualified = f"{package_name}.{module_name}"
    spec = importlib.util.spec_from_file_location(qualified, path)
    if spec is None or spec.loader is None:
        raise ProtocolError(f"cannot load trusted Needle sidecar {path.name}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[qualified] = module
    spec.loader.exec_module(module)
    return module


def _load_runtime_dependencies(model_root):
    import torch
    import transformers

    package_name = "_mayhem_trusted_needle"
    package = types.ModuleType(package_name)
    package.__path__ = [str(model_root)]
    package.__package__ = package_name
    sys.modules[package_name] = package
    configuration = _load_local_module(
        package_name,
        "configuration_needle",
        model_root / "configuration_needle.py",
    )
    tokenization = _load_local_module(
        package_name,
        "tokenization_needle",
        model_root / "tokenization_needle.py",
    )
    modeling = _load_local_module(
        package_name,
        "modeling_needle",
        model_root / "modeling_needle.py",
    )
    return torch, transformers, configuration, tokenization, modeling


def _select_device(torch):
    requested = os.environ.get("MAYHEM_NEEDLE_DEVICE", "").strip().lower()
    if requested not in {"cpu", "cuda"}:
        raise ProtocolError("MAYHEM_NEEDLE_DEVICE must be cpu or cuda")
    if requested == "cuda" and not bool(torch.cuda.is_available()):
        raise ProtocolError("CUDA was requested for Needle but is unavailable")
    return requested


def _configure_determinism(torch, device):
    torch.set_grad_enabled(False)
    if hasattr(torch, "manual_seed"):
        torch.manual_seed(0)
    if device == "cuda":
        if os.environ.get("CUBLAS_WORKSPACE_CONFIG") != ":4096:8":
            raise ProtocolError(
                "CUDA Needle requires CUBLAS_WORKSPACE_CONFIG=:4096:8"
            )
        torch.cuda.manual_seed_all(0)
        if hasattr(torch.backends, "cuda"):
            torch.backends.cuda.matmul.allow_tf32 = False
        if hasattr(torch.backends, "cudnn"):
            torch.backends.cudnn.allow_tf32 = False
            torch.backends.cudnn.benchmark = False
            torch.backends.cudnn.deterministic = True
    if hasattr(torch, "use_deterministic_algorithms"):
        torch.use_deterministic_algorithms(True)


def _handle_load(payload):
    global _runtime
    payload = _exact_object(
        payload,
        ("cache_root", "expected_sha256", "model_root"),
        "load payload",
    )
    _offline_environment()
    model_root = pathlib.Path(payload["model_root"])
    cache_root = pathlib.Path(payload["cache_root"])
    expected_sha256 = payload["expected_sha256"]
    if not model_root.is_absolute() or not cache_root.is_absolute():
        raise ProtocolError("model_root and cache_root must be absolute")
    if not isinstance(expected_sha256, dict) or set(expected_sha256) != set(
        REQUIRED_MODEL_FILES
    ):
        raise ProtocolError("expected_sha256 must bind every required Needle file")
    observed_sha256 = {}
    for name in REQUIRED_MODEL_FILES:
        path = model_root / name
        if not _real_regular_file(path):
            raise ProtocolError(
                f"trusted Needle model root is missing regular non-symlink file {name}"
            )
        expected = expected_sha256[name]
        if (
            not isinstance(expected, str)
            or re.fullmatch(r"[0-9a-f]{64}", expected) is None
        ):
            raise ProtocolError(f"expected SHA-256 for {name} is invalid")
        observed = _sha256_file(path)
        if observed != expected:
            raise ProtocolError(f"trusted Needle file changed before worker load: {name}")
        observed_sha256[name] = observed

    config_value = json.loads(
        (model_root / "config.json").read_text(encoding="utf-8"),
        parse_constant=_reject_constant,
    )
    if (
        not isinstance(config_value, dict)
        or config_value.get("model_type") != "needle"
        or config_value.get("vocab_size") != 8192
        or config_value.get("is_encoder_decoder") is not True
    ):
        raise ProtocolError("trusted Needle config does not describe the pinned architecture")

    with contextlib.redirect_stdout(sys.stderr):
        torch, transformers, configuration, tokenization, modeling = (
            _load_runtime_dependencies(model_root)
        )
        device = _select_device(torch)
        _configure_determinism(torch, device)
        dtype = (
            torch.bfloat16
            if device == "cuda"
            and bool(getattr(torch.cuda, "is_bf16_supported", lambda: False)())
            else torch.float32
        )
        config = configuration.NeedleConfig.from_pretrained(
            str(model_root),
            local_files_only=True,
        )
        tokenizer = tokenization.NeedleTokenizer.from_pretrained(
            str(model_root),
            local_files_only=True,
        )
        model = modeling.NeedleForCausalLM.from_pretrained(
            str(model_root),
            config=config,
            local_files_only=True,
            use_safetensors=True,
            torch_dtype=dtype,
        )
        model.to(device)
        model.eval()

    if (
        int(config.vocab_size) != 8192
        or int(tokenizer.vocab_size) != 8192
        or int(tokenizer.tools_token_id) != 5
        or int(tokenizer.eos_token_id) != 1
    ):
        raise ProtocolError("loaded Needle tokenizer/config do not match the pinned model")
    _runtime = {
        "cache_root": cache_root,
        "device": device,
        "dtype": str(dtype).removeprefix("torch."),
        "model": model,
        "model_root": model_root,
        "observed_sha256": observed_sha256,
        "tokenizer": tokenizer,
        "torch": torch,
        "transformers": transformers,
    }
    return {
        "execution_config": {
            "api_version": API_VERSION,
            "greedy_decoding_only": True,
            "device": device,
            "dtype": _runtime["dtype"],
            "max_decoder_tokens": MAX_DECODER_TOKENS,
            "max_encoder_tokens": MAX_ENCODER_TOKENS,
            "model_revision": MODEL_REVISION,
            "source_commit": SOURCE_COMMIT,
            "torch_version": str(torch.__version__),
            "transformers_version": str(transformers.__version__),
            "trusted_file_sha256": observed_sha256,
        },
        "n_ctx_train": MAX_ENCODER_TOKENS,
        "n_vocab": int(tokenizer.vocab_size),
    }


def _synchronize():
    if _runtime["device"] == "cuda":
        _runtime["torch"].cuda.synchronize()


def _validate_generate_payload(payload):
    payload = _exact_object(
        payload,
        (
            "frequency_penalty",
            "ignore_eos",
            "max_new_tokens",
            "min_p",
            "parallel_tool_calls",
            "presence_penalty",
            "prompt",
            "repeat_penalty",
            "seed",
            "stop",
            "temperature",
            "tools",
            "top_k",
            "top_p",
        ),
        "generate payload",
    )
    max_new_tokens = _integer(
        payload["max_new_tokens"],
        "max_new_tokens",
        1,
        MAX_DECODER_TOKENS,
    )
    if payload["temperature"] not in (None, 0, 0.0):
        raise ProtocolError("Needle supports deterministic greedy temperature=0 only")
    if payload["top_p"] not in (None, 1, 1.0):
        raise ProtocolError("Needle does not support top_p sampling")
    if payload["top_k"] not in (None, 0, 1):
        raise ProtocolError("Needle does not support top_k sampling")
    if payload["min_p"] not in (None, 0, 0.0):
        raise ProtocolError("Needle does not support min_p sampling")
    if payload["repeat_penalty"] not in (None, 1, 1.0):
        raise ProtocolError("Needle does not support repetition penalties")
    if payload["frequency_penalty"] not in (None, 0, 0.0):
        raise ProtocolError("Needle does not support frequency penalties")
    if payload["presence_penalty"] not in (None, 0, 0.0):
        raise ProtocolError("Needle does not support presence penalties")
    if payload["stop"] not in (None, []):
        raise ProtocolError("Needle uses its pinned EOS and does not support custom stops")
    if payload["ignore_eos"] is not False:
        raise ProtocolError("Needle cannot ignore EOS")
    seed = payload["seed"]
    if seed is not None:
        seed = _integer(seed, "seed", 0, (1 << 32) - 1)
    parallel_tool_calls = payload["parallel_tool_calls"]
    if parallel_tool_calls is None:
        parallel_tool_calls = True
    elif not isinstance(parallel_tool_calls, bool):
        raise ProtocolError("parallel_tool_calls must be a boolean")
    return max_new_tokens, seed, parallel_tool_calls


def _handle_generate(payload):
    if _runtime is None:
        raise ProtocolError("Needle model is not loaded")
    max_new_tokens, seed, parallel_tool_calls = _validate_generate_payload(payload)
    normalized_tools, tool_map = normalize_openai_tools(payload["tools"])
    encoder_tokens, _ = build_encoder_tokens(
        _runtime["tokenizer"],
        payload["prompt"],
        normalized_tools,
    )

    torch = _runtime["torch"]
    model = _runtime["model"]
    tokenizer = _runtime["tokenizer"]
    device = _runtime["device"]
    input_ids = torch.tensor([encoder_tokens], dtype=torch.long, device=device)
    attention_mask = torch.ones_like(input_ids)
    if seed is not None:
        torch.manual_seed(seed)
        if device == "cuda":
            torch.cuda.manual_seed_all(seed)
    _synchronize()
    started = time.perf_counter()
    with torch.inference_mode():
        encoder_hidden, encoder_mask = model.cactus_source_encode(
            input_ids,
            attention_mask,
        )
        cross_kv = model.cactus_decoder_cross_kv(encoder_hidden, encoder_mask)
    _synchronize()
    encoded_at = time.perf_counter()

    generated = []
    stopped = False
    first_call = None
    first_token_at = None
    with torch.inference_mode():
        for _ in range(max_new_tokens):
            decoder_ids = torch.tensor(
                [[int(tokenizer.eos_token_id)] + generated],
                dtype=torch.long,
                device=device,
            )
            position_ids = torch.arange(
                decoder_ids.shape[1],
                dtype=torch.long,
                device=device,
            ).unsqueeze(0)
            logits = model.cactus_decoder_step(
                decoder_ids,
                position_ids,
                encoder_mask,
                *cross_kv,
            )
            next_token = int(torch.argmax(logits[0, -1]).item())
            _synchronize()
            if first_token_at is None:
                first_token_at = time.perf_counter()
            if next_token == int(tokenizer.eos_token_id):
                stopped = True
                break
            generated.append(next_token)
            if not parallel_tool_calls:
                partial_text = tokenizer.decode(
                    generated,
                    skip_special_tokens=False,
                )
                first_call = first_complete_tool_call(partial_text, tool_map)
                if first_call is not None:
                    stopped = True
                    break
    _synchronize()
    finished = time.perf_counter()

    if first_call is None:
        raw_text = tokenizer.decode(generated, skip_special_tokens=False)
        calls = parse_and_validate_calls(raw_text, tool_map)
    else:
        calls = [first_call]
    if not parallel_tool_calls and len(calls) != 1:
        raise ProtocolError(
            "Needle non-parallel generation must produce exactly one tool call"
        )
    text = _canonical_json(calls)
    generation_ms = max(0.0, (finished - encoded_at) * 1000.0)
    prompt_eval_ms = max(0.0, (encoded_at - started) * 1000.0)
    return {
        "completion_tokens": len(generated),
        "finish_reason": "stop" if stopped else "length",
        "generation_ms": generation_ms,
        "output_text": text,
        "output_tokens_per_second": (
            len(generated) * 1000.0 / generation_ms if generation_ms > 0 else 0.0
        ),
        "prefill_tokens_per_second": (
            len(encoder_tokens) * 1000.0 / prompt_eval_ms
            if prompt_eval_ms > 0
            else 0.0
        ),
        "prompt_eval_ms": prompt_eval_ms,
        "prompt_tokens": len(encoder_tokens),
        "token_ids": generated,
        "time_to_first_token_ms": (
            max(0.0, (first_token_at - started) * 1000.0)
            if first_token_at is not None
            else max(0.0, (finished - started) * 1000.0)
        ),
    }


def _handle_tokenize(payload):
    if _runtime is None:
        raise ProtocolError("Needle model is not loaded")
    payload = _exact_object(payload, ("text",), "tokenize payload")
    text = payload["text"]
    if not isinstance(text, str) or len(text.encode("utf-8")) > MAX_PROMPT_BYTES:
        raise ProtocolError("tokenize text must be a bounded string")
    return {"token_ids": _token_ids(_runtime["tokenizer"], text)}


def _emit(value):
    encoded = json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    )
    sys.stdout.write(encoded)
    sys.stdout.write("\n")
    sys.stdout.flush()


def _handle_message(message):
    message = _exact_object(message, ("id", "op", "payload"), "worker message")
    message_id = _integer(message["id"], "message id")
    operation = message["op"]
    if not isinstance(operation, str):
        raise ProtocolError("worker operation must be a string")
    if operation == "load":
        result = _handle_load(message["payload"])
    elif operation == "generate":
        result = _handle_generate(message["payload"])
    elif operation == "tokenize":
        result = _handle_tokenize(message["payload"])
    elif operation == "shutdown":
        if message["payload"] is not None:
            raise ProtocolError("shutdown payload must be null")
        result = {"shutdown": True}
    else:
        raise ProtocolError(f"unsupported worker operation {operation!r}")
    return message_id, operation, result


def main():
    while True:
        line = sys.stdin.buffer.readline(MAX_PROTOCOL_LINE_BYTES + 1)
        if not line:
            return
        if len(line) > MAX_PROTOCOL_LINE_BYTES or not line.endswith(b"\n"):
            _emit(
                {
                    "error": "worker request exceeded its protocol bound",
                    "id": 0,
                    "ok": False,
                }
            )
            return
        message_id = 0
        try:
            message = json.loads(line, parse_constant=_reject_constant)
            if isinstance(message, dict):
                candidate = message.get("id")
                if isinstance(candidate, int) and not isinstance(candidate, bool):
                    message_id = candidate
            message_id, operation, result = _handle_message(message)
            _emit({"id": message_id, "ok": True, "result": result})
            if operation == "shutdown":
                return
        except Exception as error:
            _emit({"error": str(error), "id": message_id, "ok": False})


if __name__ == "__main__":
    main()
