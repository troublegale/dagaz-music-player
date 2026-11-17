import http from 'node:http';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';

export function resolveRange(header, size) {
  if (header === undefined) return {start:0,end:size-1,partial:false};
  const match = /^bytes=(\d*)-(\d*)$/.exec(header);
  if (!match || (!match[1] && !match[2])) return null;
  let start, end;
  if (!match[1]) {
    const suffix = Number(match[2]);
    if (!Number.isSafeInteger(suffix) || suffix <= 0) return null;
    start = Math.max(0,size-suffix); end = size-1;
  } else {
    start = Number(match[1]); end = match[2] ? Number(match[2]) : size-1;
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start >= size || end < start) return null;
    end = Math.min(end,size-1);
  }
  return {start,end,partial:true};
}

function tone() {
  const frames = 44100*30;
  const buffer = Buffer.alloc(44+frames*2);
  buffer.write('RIFF'); buffer.writeUInt32LE(buffer.length-8,4); buffer.write('WAVEfmt ',8);
  buffer.writeUInt32LE(16,16); buffer.writeUInt16LE(1,20); buffer.writeUInt16LE(1,22);
  buffer.writeUInt32LE(44100,24); buffer.writeUInt32LE(88200,28); buffer.writeUInt16LE(2,32); buffer.writeUInt16LE(16,34);
  buffer.write('data',36); buffer.writeUInt32LE(frames*2,40);
  for (let i=0;i<frames;i++) buffer.writeInt16LE(Math.round(Math.sin(2*Math.PI*440*i/44100)*12000),44+i*2);
  return buffer;
}

export function createProbeServer() {
  const files = new Map([
    ['/long.wav', tone()],
    ['/vbr.mp3', readFileSync(new URL('../../fixtures/audio/vbr.mp3',import.meta.url))],
    ['/tone.flac', readFileSync(new URL('../../fixtures/audio/tone.flac',import.meta.url))],
  ]);
  const dropped = new Set();
  return http.createServer((req,res) => {
    let url;
    try { url = new URL(req.url,'http://localhost'); } catch { res.writeHead(400).end(); return; }
    if (!['GET','HEAD'].includes(req.method)) { res.writeHead(405,{Allow:'GET, HEAD'}).end(); return; }
    const data = files.get(url.pathname);
    if (!data) { res.writeHead(404).end(); return; }
    const etag = `"${createHash('sha256').update(data).digest('hex')}"`;
    const range = resolveRange(req.headers['if-range'] && req.headers['if-range'] !== etag ? undefined : req.headers.range,data.length);
    if (!range) { res.writeHead(416,{'Content-Range':`bytes */${data.length}`}).end(); return; }
    const {start,end,partial} = range;
    const headers = {'Content-Type':url.pathname.endsWith('.wav')?'audio/wav':url.pathname.endsWith('.mp3')?'audio/mpeg':'audio/flac',
      'Content-Length':end-start+1,'Accept-Ranges':'bytes',ETag:etag};
    if (partial) headers['Content-Range'] = `bytes ${start}-${end}/${data.length}`;
    res.writeHead(partial?206:200,headers);
    if (req.method === 'HEAD') { res.end(); return; }
    let cursor = start, timer;
    const drop = url.searchParams.get('drop') === '1' && !dropped.has(url.href);
    const send = () => {
      if (res.destroyed) return;
      if (drop && cursor-start >= 512*1024) { dropped.add(url.href); res.destroy(); return; }
      if (cursor>end) { res.end(); return; }
      const next = Math.min(cursor+16384,end+1);
      const ready = res.write(data.subarray(cursor,next)); cursor=next;
      if (ready) timer=setTimeout(send,25);
      else res.once('drain',()=>{timer=setTimeout(send,25);});
    };
    res.once('close',()=>clearTimeout(timer));
    send();
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  createProbeServer().listen(18888,'127.0.0.1',()=>console.log('Probe audio on http://127.0.0.1:18888'));
}
