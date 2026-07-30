import _ from "lodash"

/**
 * Checks if `value` is considered defined akin to RoR `#defined?`.
 * 
 * @static
 * @param {*} value The value to check.
 * @returns {boolean} Returns `false` if `value` is nullish, else `true`.
 * @example
 *
 * isDefined(undefined);
 * // => false
 *
 * isDefined(null);
 * // => false
 *
 * isDefined(void 0);
 * // => false
 *
 * isDefined(NaN);
 * // => false
 */
export function isDefined(value) {
    return !_.isNil(value) && !_.isNaN(value)
}

