// Deterministic paid-demand controller. No storage access, clock, model names,
// revenue, provider capacity, or previous price enters the target calculation.
export const DEMAND_CONSTANTS = Object.freeze({
  schema_version: 1,
  reference_observations: 72,
  recent_observations: 6,
  sustained_observations: 72,
  axis_percentile: 75,
  busy_percentile: 95,
  thin_reference_paid_observations: 12,
  precision: '1000000000000',
});
export const DEMAND_PRECISION = 1_000_000_000_000n;
const WINDOW = 72;
const MAX_AXES = 16;
const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const copy = (x) => JSON.parse(JSON.stringify(x));
const gcd = (a, b) => { while (b) [a, b] = [b, a % b]; return a; };
const fraction = (n, d = 1n) => {
  if (d <= 0n || n < 0n) throw new Error('Invalid demand fraction.');
  const g = gcd(n, d);
  return [n / g, d / g];
};
const add = ([a, b], [c, d]) => fraction(a * d + c * b, b * d);
const multiply = ([a, b], [c, d]) => fraction(a * c, b * d);
const divide = ([a, b], [c, d]) => fraction(a * d, b * c);
const from = ([n, d]) => fraction(BigInt(n), BigInt(d));
const pack = (x) => x.map(String);
const sum = (values) => values.reduce(add, [0n, 1n]);
const percentile = (values, p) => values.slice().sort((a, b) =>
  compare(a[0] * b[1], b[0] * a[1]))[Math.ceil(values.length * p / 100) - 1];
const positive = (usage) => Object.values(usage).some(([n]) => n !== '0');

// An observation with sessions but no usable paid units is unknown, not idle.
// Canonical empty epochs, in contrast, are complete zero-work observations.
export function demandObservation({epoch, epoch_seconds, usage}) {
  if (!Number.isSafeInteger(epoch) || epoch < 1) throw new Error('Invalid demand epoch.');
  const units = usage?.settled_usage;
  if (!Number.isSafeInteger(epoch_seconds) || epoch_seconds < 1 ||
      !units || typeof units !== 'object' || Array.isArray(units) ||
      Object.keys(units).length > MAX_AXES) return {epoch, units: null};
  const normalized = {};
  for (const unit of Object.keys(units).sort(compare)) {
    const count = units[unit];
    if (!/^[a-zA-Z0-9_-]{1,64}$/.test(unit) ||
        typeof count !== 'string' || !/^(0|[1-9][0-9]*)$/.test(count)) {
      return {epoch, units: null};
    }
    const n = BigInt(count);
    if (n > 0n) normalized[unit] = pack(fraction(n * 3600n, BigInt(epoch_seconds)));
  }
  if ((usage.session_count ?? 0) > 0 && !positive(normalized)) return {epoch, units: null};
  return {epoch, units: normalized};
}

export function newDemandState(version = 1) {
  return {schema_version: 1, reference_version: version, last_epoch: 0,
    status: 'awaiting_paid_observation', warmup: [], reference: null, history: []};
}

function aggregate(units, scales) {
  return divide(sum(Object.entries(scales).map(([u, scale]) =>
    divide(from(units[u] ?? ['0', '1']), from(scale)))), [BigInt(Object.keys(scales).length), 1n]);
}

export function buildDemandReference(window) {
  if (window.length !== WINDOW || !positive(window[0].units)) {
    throw new Error('Demand reference requires 72 usable observations starting with paid work.');
  }
  const axes = [...new Set(window.flatMap((row) => Object.keys(row.units)))].sort(compare);
  if (!axes.length || axes.length > MAX_AXES) throw new Error('Invalid demand reference axes.');
  const scales = Object.fromEntries(axes.map((unit) => [unit, pack(percentile(
    window.filter((row) => row.units[unit]).map((row) => from(row.units[unit])), 75))]));
  const paid = window.filter((row) => positive(row.units));
  const busy = percentile(paid.map((row) => aggregate(row.units, scales)), 95);
  return {
    first_epoch: window[0].epoch, last_epoch: window.at(-1).epoch,
    paid_observations: paid.length, thin: paid.length < 12,
    axis_scales: scales, busy_aggregate: pack(busy),
    effective_scales: Object.fromEntries(axes.map((u) => [u, pack(multiply(from(scales[u]), busy))])),
  };
}

export function normalizedDemand(units, reference) {
  if (units === null || Object.keys(units).some((u) => !(u in reference.effective_scales))) return null;
  const [n, d] = aggregate(units, reference.effective_scales);
  // Round once per aggregate hourly observation, after unit normalization.
  // Fixed precision bounds retained arithmetic regardless of history length.
  return n >= d ? DEMAND_PRECISION : (2n * n * DEMAND_PRECISION + d) / (2n * d);
}

export function demandTarget(history) {
  if (!history.length || history.length > WINDOW) throw new Error('Invalid demand history length.');
  const values = history.map(BigInt);
  if (values.some((x) => x < 0n || x > DEMAND_PRECISION)) throw new Error('Invalid clipped demand.');
  const recent = values.slice(-6);
  const fastSum = recent.reduce((a, b) => a + b, 0n);
  const sustainedSum = values.reduce((a, b) => a + b, 0n);
  // Keep the target rational until the final atomic price rounding.
  const target = fraction(DEMAND_PRECISION * BigInt(recent.length) + 15n * fastSum,
    4n * DEMAND_PRECISION * BigInt(recent.length));
  const ceiling = fraction(72n ** 2n * DEMAND_PRECISION ** 2n + 3n * sustainedSum ** 2n,
    72n ** 2n * DEMAND_PRECISION ** 2n);
  const multiplier = target[0] * ceiling[1] < ceiling[0] * target[1] ? target : ceiling;
  return {multiplier: pack(multiplier), ceiling: pack(ceiling),
    recent_sum: String(fastSum), recent_count: recent.length, sustained_sum: String(sustainedSum)};
}

export function advanceDemand(previous, observation) {
  const state = copy(previous ?? newDemandState());
  if (state.schema_version !== 1 || state.warmup.length > WINDOW || state.history.length > WINDOW) {
    throw new Error('Invalid demand controller state.');
  }
  if (observation.epoch <= state.last_epoch) {
    if (observation.epoch === state.last_epoch && JSON.stringify(observation) === JSON.stringify(state.last_observation)) {
      return {state, target: state.last_target ?? null, idempotent: true};
    }
    throw new Error('Conflicting or out-of-order demand observation.');
  }
  state.last_epoch = observation.epoch;
  state.last_observation = copy(observation);
  let target = null;
  if (observation.units === null) state.status = 'unknown_evidence';
  else if (!state.reference) {
    if (state.warmup.length || positive(observation.units)) state.warmup.push(copy(observation));
    state.status = state.warmup.length ? 'building_reference' : 'awaiting_paid_observation';
    if (state.warmup.length === WINDOW) {
      state.reference = buildDemandReference(state.warmup);
      state.history = state.warmup.map((row) => String(normalizedDemand(row.units, state.reference)));
      state.warmup = [];
      state.status = 'reference_ready';
      // Reference window is evidence, never an in-sample price forecast.
    }
  } else {
    const value = normalizedDemand(observation.units, state.reference);
    if (value === null) state.status = 'unrecognized_paid_axis';
    else {
      state.history = [...state.history, String(value)].slice(-WINDOW);
      state.status = 'active';
      target = demandTarget(state.history);
    }
  }
  state.last_target = target;
  return {state, target, idempotent: false};
}

export function scaleDemandPrice(amount, [numerator, denominator]) {
  const n = BigInt(amount), a = BigInt(numerator), b = BigInt(denominator);
  if (n < 0n || a < 0n || b <= 0n) throw new Error('Invalid reference price scaling.');
  if (!n) return '0';
  const value = (2n * n * a + b) / (2n * b);
  return String(value > 0n ? value : 1n);
}
