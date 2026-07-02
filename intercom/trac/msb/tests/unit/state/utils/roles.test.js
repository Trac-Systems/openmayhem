import test from 'brittle';
import roles from '../../../../src/core/state/utils/roles.js';

const { NodeRole, WHITELISTED_MASK, WRITER_MASK, INDEXER_MASK, calculateNodeRole, isNodeRoleValid } = roles;

test('NodeRole enum values', t => {
    t.is(NodeRole.READER, 0x0);
    t.is(NodeRole.WHITELISTED, 0x1);
    t.is(NodeRole.WRITER, 0x3);
    t.is(NodeRole.INDEXER, 0x7);
});

test('Bitmask constants', t => {
    t.is(WHITELISTED_MASK, 0x1);
    t.is(WRITER_MASK, 0x2);
    t.is(INDEXER_MASK, 0x4);
});

test('calculateNodeRole returns correct results', t => {
    t.is(calculateNodeRole({}), NodeRole.READER);
    t.is(calculateNodeRole({ isWhitelisted: true }), NodeRole.WHITELISTED);
    t.is(calculateNodeRole({ isWhitelisted: true, isWriter: true }), NodeRole.WRITER);
    t.is(calculateNodeRole({ isWhitelisted: true, isWriter: true, isIndexer: true }), NodeRole.INDEXER);
});

test('calculateNodeRole returns correct bitmask for partial roles', t => {
    t.is(calculateNodeRole({ isWriter: true }), NodeRole.READER | WRITER_MASK);
    t.is(calculateNodeRole({ isIndexer: true }), NodeRole.READER | INDEXER_MASK);
});

test('isNodeRoleValid returns true for valid roles', t => {
    t.ok(isNodeRoleValid(NodeRole.READER));
    t.ok(isNodeRoleValid(NodeRole.WHITELISTED));
    t.ok(isNodeRoleValid(NodeRole.WRITER));
    t.ok(isNodeRoleValid(NodeRole.INDEXER));
});

test('isNodeRoleValid returns false for invalid roles', t => {
    t.not(isNodeRoleValid(0x2));
    t.not(isNodeRoleValid(0x4));
    t.not(isNodeRoleValid(0x5));
    t.not(isNodeRoleValid(-1));
    t.not(isNodeRoleValid(999));
});
