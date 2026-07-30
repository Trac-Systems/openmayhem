import b4a from 'b4a';
import _ from 'lodash';

export const isDeepEqualApplyPayload = (left, right) => {
    if (left === right) return true;

    const leftIsBuffer = b4a.isBuffer(left);
    const rightIsBuffer = b4a.isBuffer(right);

    if (leftIsBuffer || rightIsBuffer) {
        return leftIsBuffer && rightIsBuffer && b4a.equals(left, right);
    }

    const leftIsArray = Array.isArray(left);
    const rightIsArray = Array.isArray(right);

    if (leftIsArray || rightIsArray) {
        if (!leftIsArray || !rightIsArray) return false;
        if (left.length !== right.length) return false;

        for (let i = 0; i < left.length; i++) {
            if (!isDeepEqualApplyPayload(left[i], right[i])) return false;
        }
        return true;
    }

    if (!_.isObject(left) || !_.isObject(right)) return false;

    const leftKeys = Object.keys(left);
    const rightKeys = Object.keys(right);

    if (leftKeys.length !== rightKeys.length) return false;

    for (const key of leftKeys) {
        if (!Object.prototype.hasOwnProperty.call(right, key)) return false;
        if (!isDeepEqualApplyPayload(left[key], right[key])) return false;
    }

    return true;
};
