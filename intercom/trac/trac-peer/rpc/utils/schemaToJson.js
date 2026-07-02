const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

const withNullable = (schema, nullable) => {
  if (!nullable) return schema;
  if (!schema || typeof schema !== "object") return schema;
  if (schema.type === undefined) return schema;
  const t = schema.type;
  if (Array.isArray(t)) {
    if (!t.includes("null")) return { ...schema, type: [...t, "null"] };
    return schema;
  }
  if (typeof t === "string") return { ...schema, type: [t, "null"] };
  return schema;
};


export const fastestToJsonSchema = (fv) => {
  if (fv === null || fv === undefined) return {};
  if (Array.isArray(fv)) return {};
  if (!isObject(fv)) return {};

  if (fv.$$type === "object" || fv.$$type === "Object") {
    const properties = {};
    const required = [];
    const additionalProperties = fv.$$strict === true ? false : true;

    for (const [key, sub] of Object.entries(fv)) {
      if (key.startsWith("$$")) continue;
      properties[key] = fastestToJsonSchema(sub);
      if (!(isObject(sub) && sub.optional === true)) required.push(key);
    }

    const out = { type: "object", properties, additionalProperties };
    if (required.length) out.required = required;
    return out;
  }

  // Simple typed schema  
  const type = fv.type;
  const nullable = fv.nullable === true;

  if (type === "any") return {};

  if (type === "string") {
    const out = { type: "string" };
    if (typeof fv.min === "number") out.minLength = fv.min;
    if (typeof fv.max === "number") out.maxLength = fv.max;
    if (fv.numeric === true) out.pattern = "^[0-9]+$";
    return withNullable(out, nullable);
  }

  if (type === "boolean") return withNullable({ type: "boolean" }, nullable);

  if (type === "number") {
    const out = { type: fv.integer === true ? "integer" : "number" };
    if (typeof fv.min === "number") out.minimum = fv.min;
    if (typeof fv.max === "number") out.maximum = fv.max;
    return withNullable(out, nullable);
  }

  // Custom validators used in this repo
  if (type === "is_hex") {
    const out = { type: "string", pattern: "^[0-9a-fA-F]+$" };
    if (typeof fv.min === "number") out.minLength = fv.min;
    if (typeof fv.max === "number") out.maxLength = fv.max;
    return withNullable(out, nullable);
  }

  if (type === "bitcoin_address") {
    return withNullable({ type: "string" }, nullable);
  }

  // Fallback: unknown schema fragment
  return {};
};

