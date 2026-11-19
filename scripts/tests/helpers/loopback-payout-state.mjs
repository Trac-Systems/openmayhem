// The shell deliberately accepts only http://mock.invalid/v1 in test mode.
// Route that one name to the loopback fixture for subprocess Node fetches.
const orig = globalThis.fetch;
globalThis.fetch = (input, options) => {
  const url = new URL(input);
  if (url.origin === 'http://mock.invalid') {
    url.hostname = '127.0.0.1';
    url.port = process.env.MOCK_PAYOUT_STATE_PORT;
  }
  if (url.hostname !== '127.0.0.1') throw new Error('payout fixture refuses external traffic');
  return orig(url, options);
};
