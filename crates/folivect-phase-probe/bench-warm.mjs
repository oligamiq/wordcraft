import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {Worker, isMainThread, parentPort, workerData} from 'node:worker_threads';
import {createHash} from 'node:crypto';
import {performance} from 'node:perf_hooks';

// Diagnostic only: independent Node workers emulate separately reused WASM
// isolates. These are NOT Cloudflare billable CPU or a Wizer snapshot.
const cpu = () => {
  const {user, system} = process.threadCpuUsage();
  return (user + system) / 1000;
};
const stamp = () => [cpu(), performance.now()];
const elapsed = t => ({thread_ms:cpu()-t[0], wall_ms:performance.now()-t[1]});
const golden = [
  'cc5b14c69d663932ed4a74d75357d66d3222d4156f7b05300b06cc96e8b80f49',
  'dbb7f2dcf262688e485a95bd1db55c247131eec244b5713b6d3962b7afbfa918',
];
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
// Default remains the pinned 120-paragraph suite. Scaling diagnostics are
// bounded and explicitly separate from the baseline time-series.
const scaling = process.argv[3] === '--scale';
if (process.argv[3] && !scaling) throw Error('Unknown benchmark mode');
const suite = scaling ? [0,1,2,3,4,5,6,7] : [0,1];
const warmups = scaling ? 3 : 12;
const samples = scaling ? 12 : 32;
const stats = values => {
  assert.equal(values.length,samples);
  const a=[...values].sort((x,y)=>x-y);
  assert.ok(a.every(x=>Number.isFinite(x)&&x>=0));
  return {n:a.length,mean:a.reduce((s,x)=>s+x,0)/a.length,p95:a[Math.ceil(a.length*.95)-1],max:a.at(-1)};
};

if (!isMainThread) {
  const {role,module,glue} = workerData;
  const api = await import(glue);
  let t = stamp();
  const instance = api.initSync({module});
  const init = elapsed(t);
  t = stamp();
  api.prewarm_public_fonts();
  const fonts = elapsed(t);
  parentPort.postMessage({ready:true,role,init,fonts,memory_bytes:instance.memory.buffer.byteLength});
  const checkedBatches = new Set();
  parentPort.on('message', ({id,fixture,packet}) => {
    const phases={};
    let doc, probe, bytes;
    try {
      const total=stamp();
      let phase=stamp();
      doc=api.DocumentProbe.from_fixture(fixture);
      phases.document=elapsed(phase);
      phase=stamp();
      if (role === 'consumer') probe=doc.decode(3,packet);
      else probe=doc.layout();
      phases[role === 'consumer'?'decode':'layout']=elapsed(phase);
      const pages=probe.pages();
      phase=stamp();
      bytes=role === 'producer'?probe.packet(3):probe.pdf();
      phases[role === 'producer'?'encode':'pdf']=elapsed(phase);
      phase=stamp();
      probe.free();
      doc.free();
      probe=null;
      doc=null;
      phases.dispose=elapsed(phase);
      phases.request=elapsed(total);
      // Verify batching outside the CPU measurement on the first (warmup)
      // request per fixture only. The production-equivalent phase values
      // must not include extra verification work.
      if (role==='producer' && workerData.verifyBatches && !checkedBatches.has(fixture)) {
        const expected=hash(bytes);
        const vdoc=api.DocumentProbe.from_fixture(fixture);
        const vprobe=vdoc.layout();
        try {
          for (const limit of [1,2,3,7]) {
            assert.equal(hash(vprobe.packet_batched(limit)),expected,`page batch ${limit} changed packet`);
          }
          assert.throws(() => vprobe.packet_batched(0));
          checkedBatches.add(fixture);
        } finally {vprobe.free();vdoc.free();}
      }
      parentPort.postMessage({id,role,pages,phases,bytes},[bytes.buffer]);
    } catch(e) {
      try {probe?.free();doc?.free();} catch {}
      parentPort.postMessage({id,error:String(e),stack:e?.stack});
    }
  });
} else {
  const dir = path.resolve(process.argv[2]);
  assert.ok(fs.existsSync(path.join(dir,'folivect_phase_probe_bg.wasm')));
  const module = new WebAssembly.Module(fs.readFileSync(path.join(dir,'folivect_phase_probe_bg.wasm')));
  const glue=pathToFileURL(path.join(dir,'folivect_phase_probe.js')).href;
  const workers={};
  const setup={};
  let nextId=0;
  async function create(role) {
    const worker = new Worker(new URL(import.meta.url),{workerData:{module,glue,role,verifyBatches:scaling}});
    const ready = await new Promise((resolve,reject)=>{
      const onMessage=x=>{if(x.ready){worker.off('message',onMessage);resolve(x);}};
      worker.on('message',onMessage);
      worker.once('error',reject);
      worker.once('exit',code=>{if(code!==0)reject(Error(`Worker ${role} exited ${code} before ready`));});
    });
    workers[role]=worker;
    setup[role]=ready;
    console.log('WASM_REUSED_ISOLATE_INIT',JSON.stringify(ready));
  }
  async function call(role,fixture,packet) {
    const w=workers[role];
    const id=++nextId;
    return new Promise((resolve,reject)=>{
      const onMessage=x=>{
        if(x.id!==id)return;
        w.off('message',onMessage);
        if(x.error)reject(Error(x.error+'\n'+x.stack));
        else resolve(x);
      };
      w.on('message',onMessage);
      w.postMessage({id,fixture,packet});
    });
  }
  try {
    for(const role of ['producer','consumer','mono'])await create(role);
    for(const fixture of suite) {
      const reference=await call('mono',fixture);
      assert.ok(reference.pages>0,'no pages');
      const expectedHash=hash(reference.bytes);
      if(fixture<golden.length) assert.equal(expectedHash,golden[fixture],'original pinned PDF changed');
      const records={producer:[],consumer:[],mono:[]};
      for(let i=-warmups;i<samples;i++){
        // Alternate the mono baseline relative to the two-stage pipeline.
        let p,c,m;
        if(i%2===0)m=await call('mono',fixture);
        p=await call('producer',fixture);
        c=await call('consumer',fixture,p.bytes);
        if(i%2!==0)m=await call('mono',fixture);
        for(const x of [p,c,m]){
          assert.equal(x.pages,reference.pages,'page count drift');
          if(x.role!=='producer')assert.equal(hash(x.bytes),expectedHash,`PDF mismatch ${fixture}:${x.role}`);
          if(i>=0)records[x.role].push(x.phases);
        }
      }
      for(const role of ['producer','consumer','mono']){
        const r=records[role];
        assert.equal(r.length,samples);
        const phases=Object.fromEntries(Object.keys(r[0]).map(key=>[key,{
          thread_ms:stats(r.map(x=>x[key].thread_ms)),
          wall_ms:stats(r.map(x=>x[key].wall_ms)),
        }]));
        console.log(scaling?'WASM_SCALING_CPU':'WASM_REUSED_ISOLATE_CPU',JSON.stringify({
          fixture,paragraphs:120*(2**Math.floor(fixture/2)),role,pages:reference.pages,
          sha256:expectedHash,phases,
        }));
      }
    }
  } finally {
    await Promise.all(Object.values(workers).map(w=>w.terminate()));
  }
}
