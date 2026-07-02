export function getConfirmedParameter(url, { defaultValue = true } = {}) {
    const confirmedParam = url.searchParams.get("confirmed");

    if (confirmedParam === null) {
        return defaultValue;
    }

    if (confirmedParam === "true") {
        return true;
    }

    if (confirmedParam === "false") {
        return false;
    }

    return null;
}
