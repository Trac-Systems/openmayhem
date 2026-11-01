export function fakeIntervals(t) {
    const originalSet = globalThis.setInterval;
    const originalClear = globalThis.clearInterval;
    const active = new Map();
    let unrefs = 0;
    globalThis.setInterval = (callback, ms) => {
        const timer = { unref() { unrefs++; } };
        active.set(timer, { callback, ms });
        return timer;
    };
    globalThis.clearInterval = timer => active.delete(timer);
    t.teardown(() => {
        globalThis.setInterval = originalSet;
        globalThis.clearInterval = originalClear;
    });
    return {
        active,
        get unrefs() { return unrefs; },
        tick() { for (const { callback } of [...active.values()]) callback(); },
    };
}
