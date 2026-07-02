import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import OpenAI from 'openai';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const crateDir = path.resolve(scriptDir, '..');
const repoRoot = path.resolve(crateDir, '../..');

const freePort = () =>
  new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      server.close(() => resolve(port));
    });
  });

const waitForGateway = async (baseUrl, child) => {
  const deadline = Date.now() + 60_000;
  let lastError = null;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`gateway exited early with code ${child.exitCode}`);
    }
    try {
      const response = await fetch(`${baseUrl}/mayhem/status`);
      if (response.ok) return;
      lastError = new Error(`status ${response.status}`);
    } catch (err) {
      lastError = err;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`gateway did not become ready: ${lastError?.message ?? 'timeout'}`);
};

const startGateway = async () => {
  const port = await freePort();
  const bind = `127.0.0.1:${port}`;
  const child = spawn('cargo', ['run', '-p', 'mayhem-gateway', '--', '--bind', bind], {
    cwd: repoRoot,
    env: { ...process.env, MAYHEM_GATEWAY_BIND: bind },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.on('data', () => {});
  child.stderr.on('data', () => {});
  const baseUrl = `http://${bind}`;
  await waitForGateway(baseUrl, child);
  return { child, baseUrl };
};

const stopGateway = async (child) => {
  if (!child || child.exitCode !== null) return;
  child.kill('SIGTERM');
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, 5000);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
  });
  if (child.exitCode === null) child.kill('SIGKILL');
};

const collectChatStream = async (stream) => {
  let content = '';
  let usage = null;
  let chunks = 0;
  for await (const chunk of stream) {
    chunks += 1;
    content += chunk.choices?.[0]?.delta?.content ?? '';
    if (chunk.usage) usage = chunk.usage;
  }
  return { content, usage, chunks };
};

const maybeRemoteReferenceCheck = async (localChunkShape) => {
  const baseURL = process.env.MAYHEM_REMOTE_OPENAI_BASE_URL;
  const apiKey = process.env.MAYHEM_REMOTE_OPENAI_API_KEY;
  const model = process.env.MAYHEM_REMOTE_OPENAI_MODEL;
  if (!baseURL || !apiKey || !model) return false;

  const remote = new OpenAI({ apiKey, baseURL });
  const stream = await remote.chat.completions.create({
    model,
    messages: [{ role: 'user', content: 'Say ok.' }],
    stream: true,
    max_tokens: 4,
  });
  let sawChunk = false;
  for await (const chunk of stream) {
    sawChunk = true;
    assert.equal(typeof chunk.id, 'string');
    assert.equal(chunk.object, localChunkShape.object);
    assert.ok(Array.isArray(chunk.choices));
    break;
  }
  assert.equal(sawChunk, true, 'remote reference returned no streaming chunks');

  const toolResponse = await remote.chat.completions.create({
    model,
    messages: [{ role: 'user', content: 'Call the get_weather tool.' }],
    tools: [
      {
        type: 'function',
        function: {
          name: 'get_weather',
          description: 'Get weather',
          parameters: { type: 'object', properties: {} },
        },
      },
    ],
    tool_choice: { type: 'function', function: { name: 'get_weather' } },
    max_tokens: 32,
  });
  const toolCall = toolResponse.choices?.[0]?.message?.tool_calls?.[0];
  assert.equal(toolCall?.type, 'function');
  assert.equal(toolCall?.function?.name, 'get_weather');
  assert.equal(typeof toolCall?.function?.arguments, 'string');
  return true;
};

const { child, baseUrl } = await startGateway();
try {
  const client = new OpenAI({ apiKey: 'mayhem-local-test', baseURL: `${baseUrl}/v1` });

  const models = await client.models.list();
  assert.equal(models.object, 'list');
  assert.ok(models.data.length > 0);
  assert.equal(models.data[0].object, 'model');
  const model = models.data[0].id;

  const toolResponse = await client.chat.completions.create({
    model,
    messages: [{ role: 'user', content: 'Call the weather tool.' }],
    tools: [
      {
        type: 'function',
        function: {
          name: 'get_weather',
          description: 'Get weather',
          parameters: { type: 'object', properties: {} },
        },
      },
    ],
  });
  const toolCall = toolResponse.choices[0].message.tool_calls?.[0];
  assert.equal(toolResponse.choices[0].finish_reason, 'tool_calls');
  assert.equal(toolCall?.function?.name, 'get_weather');

  const followup = await client.chat.completions.create({
    model,
    messages: [
      { role: 'user', content: 'Call the weather tool.' },
      toolResponse.choices[0].message,
      { role: 'tool', tool_call_id: toolCall.id, content: '{"temperature_c":21}' },
    ],
  });
  assert.match(followup.choices[0].message.content ?? '', /temperature_c/);

  const stream = await client.chat.completions.create({
    model,
    messages: [{ role: 'user', content: 'Stream a short answer.' }],
    stream: true,
    stream_options: { include_usage: true },
  });
  const streamed = await collectChatStream(stream);
  assert.match(streamed.content, /Mayhem response/);
  assert.ok(streamed.usage?.total_tokens >= 1);
  assert.ok(streamed.chunks >= 2);

  const completion = await client.completions.create({
    model,
    prompt: 'legacy prompt',
    max_tokens: 8,
  });
  assert.equal(completion.object, 'text_completion');
  assert.match(completion.choices[0].text, /Mayhem completion/);

  const curl = spawnSync('curl', ['-sfS', `${baseUrl}/v1/models`], { encoding: 'utf8' });
  assert.equal(curl.status, 0, curl.stderr);
  assert.equal(JSON.parse(curl.stdout).object, 'list');

  const checkedRemote = await maybeRemoteReferenceCheck({
    object: 'chat.completion.chunk',
  });
  if (checkedRemote) {
    console.log('remote reference streaming and tool-call shapes matched');
  }

  console.log('OpenAI SDK and curl smoke passed');
} finally {
  await stopGateway(child);
}
