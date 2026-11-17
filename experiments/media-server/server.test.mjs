import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { resolveRange, createProbeServer } from './server.mjs';

test('single byte ranges include suffixes and reject invalid boundaries', () => {
  assert.deepEqual(resolveRange('bytes=2-5',10),{start:2,end:5,partial:true});
  assert.deepEqual(resolveRange('bytes=-2',10),{start:8,end:9,partial:true});
  assert.deepEqual(resolveRange('bytes=8-',10),{start:8,end:9,partial:true});
  for (const header of ['bytes=10-','bytes=5-2','bytes=-0','bytes=0-1,4-5','bytes=-','bytes=999999999999999999999-']) assert.equal(resolveRange(header,10),null);
});

test('HTTP streams WAV bytes with Range and If-Range semantics', async t => {
  const server = createProbeServer();
  server.listen(0,'127.0.0.1');
  await once(server,'listening');
  t.after(() => new Promise(resolve => server.close(resolve)));
  const base = `http://127.0.0.1:${server.address().port}`;
  const response = await fetch(`${base}/long.wav`,{headers:{Range:'bytes=0-43'}});
  assert.equal(response.status,206);
  assert.equal(response.headers.get('content-range'),'bytes 0-43/2646044');
  const bytes = Buffer.from(await response.arrayBuffer());
  assert.equal(bytes.toString('ascii',0,4),'RIFF');
  assert.equal(bytes.length,44);
  const mismatch = await fetch(`${base}/long.wav`,{method:'HEAD',headers:{Range:'bytes=0-43','If-Range':'wrong'}});
  assert.equal(mismatch.status,200);
  assert.equal(mismatch.headers.get('content-length'),'2646044');
  assert.equal((await fetch(`${base}/long.wav`,{headers:{Range:'bytes=5-2'}})).status,416);
  assert.equal((await fetch(`${base}/unknown`)).status,404);
});

test('injected disconnect is followed by a resumable range on the same entity', async t => {
  const server = createProbeServer();
  server.listen(0,'127.0.0.1');
  await once(server,'listening');
  t.after(() => new Promise(resolve => server.close(resolve)));
  const url = `http://127.0.0.1:${server.address().port}/long.wav?drop=1`;
  const first = await fetch(url);
  const etag = first.headers.get('etag');
  await assert.rejects(first.arrayBuffer());
  const resumed = await fetch(url,{headers:{Range:'bytes=524288-524331','If-Range':etag}});
  assert.equal(resumed.status,206);
  assert.equal(resumed.headers.get('etag'),etag);
  assert.equal(resumed.headers.get('content-range'),'bytes 524288-524331/2646044');
  assert.equal((await resumed.arrayBuffer()).byteLength,44);
});
