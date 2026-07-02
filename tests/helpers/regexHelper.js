export function errorMessageIncludes(substring) {
    return new RegExp(substring.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
}