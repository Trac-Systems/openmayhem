function normalizedModelIdentifier(value) {
  return String(value || '').trim().toLowerCase().replace(/[^a-z0-9]+/g, '');
}

function stringValues(value) {
  if (typeof value === 'string') return [value];
  if (!Array.isArray(value)) return [];
  return value.filter((entry) => typeof entry === 'string');
}

export function catalogModelIdentifiers(model) {
  return [...new Set(
    [
      model?.model_id,
      ...stringValues(model?.alias),
      ...stringValues(model?.aliases),
      ...stringValues(model?.model_aliases),
    ]
      .map(normalizedModelIdentifier)
      .filter(Boolean)
  )];
}

export function parseLaunchRows(section) {
  const rows = [];
  for (const line of String(section || '').split('\n')) {
    const cells = line
      .trim()
      .replace(/^\|/, '')
      .replace(/\|$/, '')
      .split('|')
      .map((cell) => cell.trim());
    if (cells.length < 4) continue;

    const identifiers = [...cells[0].matchAll(/`([^`\r\n]+)`/g)]
      .map((match) => match[1].trim())
      .filter(Boolean);
    if (identifiers.length === 0) continue;
    rows.push({
      id: identifiers[0],
      aliases: identifiers.slice(1),
      identifiers,
      status: cells[3].replace(/[*_`]/g, '').trim().toLowerCase(),
      cells,
      line,
    });
  }
  return rows;
}

export function launchRowsMatchingModel(rows, model) {
  const expected = new Set(catalogModelIdentifiers(model));
  const exact = rows.filter((row) =>
    row.identifiers.some((identifier) =>
      expected.has(normalizedModelIdentifier(identifier))
    )
  );
  if (exact.length > 0) return exact;

  const familyAlias = normalizedModelIdentifier(model?.family);
  if (familyAlias.length === 0) return [];
  return rows.filter((row) =>
    row.identifiers.some((identifier) =>
      normalizedModelIdentifier(identifier).includes(familyAlias)
    )
  );
}

export function launchRowsIncludeModel(rows, model) {
  return launchRowsMatchingModel(rows, model).length > 0;
}
