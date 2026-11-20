import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {Worker, isMainThread, parentPort, workerData} from 'node:worker_threads';
import {createHash} from 'node:crypto';
import {performance} from 'node:perf_hooks';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const cpu = () => {
  const {user, system} = process.threadCpuUsage();
  return (user + system) / 1000;
};
const stamp = () => [cpu(), performance.now()];
const elapsed = t => ({thread:cpu()-t[0], wall:performance.now()-t[1]});
const names = ['json','msgpack','interned'];

if (!isMainThread) {
  const {module, glue, fixture, codec, mode, packet} = workerData;
  const api = await import(glue);
  const phases = {};
  let probe, output, count, instance;
  const total = stamp();
  let t = stamp();
  instance = api.initSync({module});
  phases.init = elapsed(t);
  t = stamp();
  api.prewarm_public_fonts();
  phases.fonts = elapsed(t);
  try {
    t = stamp();
    if (mode === 'consumer') probe = api.Probe.from_packet(fixture, codec, packet);
    else probe = api.Probe.from_fixture(fixture);
    phases[mode === 'consumer' ? 'decode_doc' : 'layout_doc'] = elapsed(t);
    count = probe.pages();
    t = stamp();
    output = mode === 'producer' ? probe.packet(codec) : probe.pdf();
    phases[mode === 'producer' ? 'encode' : 'pdf'] = elapsed(t);
  } finally {
    t = stamp();
    probe?.free();
    phases.dispose = elapsed(t);
  }
  phases.total = elapsed(total);
  // Node-owned result copies and worker transport are outside the measured operation.
  const bytes = new Uint8Array(output);
  parentPort.postMessage({phases,count,peak:instance.memory.buffer.byteLength,bytes}, [bytes.buffer]);
} else {
  assert.equal(typeof process.threadCpuUsage,'function');
  const dir = path.resolve(process.argv[2]);
  const glue = pathToFileURL(path.join(dir,'folivect_phase_probe.js')).href;
  const module = new WebAssembly.Module(fs.readFileSync(path.join(dir,'folivect_phase_probe_bg.wasm')));
  async function run(fixture, mode, codec=0, packet) {
    return await new Promise((resolve,reject) => {
      const w=new Worker(new URL(import.meta.url),{workerData:{module,glue,fixture,mode,codec,packet}});
      let result;
      w.once('message', m => {result=m;});
      w.once('error', reject);
      w.once('exit', code => code === 0 && result ? resolve(result) : reject(new Error('probe worker exit '+code)));
    });
  }
  function stats(values) {
    const a=[...values].sort((a,b)=>a-b);
    return {n:a.length,mean:a.reduce((s,x)=>s+x,0)/a.length,p95:a[Math.ceil(a.length*.95)-1]};
  }
  for (const fixture of [0,1]) {
    const reference=await run(fixture,'mono');
    assert.ok(reference.count>1);
    assert.equal(Buffer.from(reference.bytes.subarray(0,5)).toString(),'%PDF-');
    const expected=digest(reference.bytes);
    const rows=[];
    for(let i=-12;i<32;i++){
      const order=i%2===0?[0,1,2,3]:[3,2,1,0];
      for(const codec of order){
        if(codec===0) {
          const m=await run(fixture,'mono');
          assert.equal(digest(m.bytes),expected,'monolithic output changed');
          assert.equal(m.count,reference.count);
          if(i>=0) rows.push({name:'mono',...m,bytes:undefined});
        } else {
          const p=await run(fixture,'producer',codec);
          const c=await run(fixture,'consumer',codec,p.bytes);
          assert.equal(digest(c.bytes),expected,'layout-transfer output changed for '+names[codec-1]);
          assert.equal(p.count,reference.count);
          assert.equal(c.count,reference.count);
          if(i>=0) rows.push({name:names[codec-1]+'_producer',...p,bytes:undefined},{name:names[codec-1]+'_consumer',...c,bytes:undefined});
        }
      }
    }
    for(const name of [...new Set(rows.map(r=>r.name))]) {
      const entries=rows.filter(r=>r.name===name);
      const phases=Object.fromEntries(Object.keys(entries[0].phases).map(k=>[k,{thread_ms:stats(entries.map(r=>r.phases[k].thread)),wall_ms:stats(entries.map(r=>r.phases[k].wall))}]));
      console.log('WASM_PHASE_PROBE',JSON.stringify({fixture,name,pages:reference.count,sha256:expected,peak:stats(entries.map(r=>r.peak)),phases}));
    }
  }
}
