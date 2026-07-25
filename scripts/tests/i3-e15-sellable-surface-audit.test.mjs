import assert from 'node:assert/strict';

import {
  catalogModelIdentifiers,
  launchRowsIncludeModel,
  launchRowsMatchingModel,
  parseLaunchRows,
} from '../lib/launch-roster.mjs';

const rows = parseLaunchRows(`
| Model | Category | Class | Status |
|---|---|---|---|
| \`Org/Canonical-Model\` (\`@artifact\`) | LLM | A | live |
| \`friendly-model\` | Image | B | live |
| \`Publisher/Family-Description-Pro\` | Audio | C | live |
| \`unrelated/model\` | family-description-only | D | live |
`);

assert.deepEqual(rows[0].identifiers, ['Org/Canonical-Model', '@artifact']);
assert.equal(rows[0].status, 'live');
assert.deepEqual(
  catalogModelIdentifiers({
    model_id: 'org/other-model',
    alias: 'single-alias',
    aliases: ['friendly-model'],
    model_aliases: ['legacy-model'],
  }),
  ['orgothermodel', 'singlealias', 'friendlymodel', 'legacymodel'],
);
assert.equal(
  launchRowsIncludeModel(rows, {
    model_id: 'org/canonical-model',
    family: 'unrelated-family',
  }),
  true,
  'canonical model IDs should match case-insensitively',
);
assert.equal(
  launchRowsIncludeModel(rows, {
    model_id: 'org/renamed-model',
    aliases: ['friendly-model'],
    family: 'unrelated-family',
  }),
  true,
  'declared model aliases should match roster identifiers',
);
assert.equal(
  launchRowsIncludeModel(rows, {
    model_id: 'org/family-model',
    family: 'family-description',
  }),
  true,
  'catalog family aliases should match only within a model identifier',
);
assert.equal(
  launchRowsIncludeModel(rows.slice(-1), {
    model_id: 'org/missing-model',
    family: 'family-description-only',
  }),
  false,
  'family text outside the model cell must not satisfy the roster check',
);
assert.equal(
  launchRowsIncludeModel(rows, { model_id: 'org/canonicalmodel' }),
  true,
  'identifier punctuation and case should normalize consistently',
);
assert.equal(
  launchRowsIncludeModel(rows, { model_id: 'org/canonical-model-plus' }),
  false,
  'canonical model IDs must not match by partial prefix',
);
assert.deepEqual(
  launchRowsMatchingModel(rows, {
    model_id: 'org/canonical-model',
    family: 'family-description',
  }).map(({ id }) => id),
  ['Org/Canonical-Model'],
  'an exact canonical ID must take precedence over broader family aliases',
);

process.stdout.write('i3-e15-sellable-surface-audit.test: ok\n');
