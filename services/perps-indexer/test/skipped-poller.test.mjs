import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createPerpsIndexerService } from "../src/indexer.mjs";
import { ARABICA_MARKET_ID } from "../src/config.mjs";

const parent=277836282, missing=parent+1, successor=parent+2;
const header=(height,prev_height,hash,prev_hash)=>({height,prev_height,hash,prev_hash,timestamp_nanosec:"1794696152000000000"});
const p=header(parent,parent-1,"parent","older");
const n=header(successor,parent,"successor","parent");
const after=header(successor+1,successor,"after","successor");
for(const mode of ["valid","bad-parent","unverified","wrong-payload","rate-limit"]){
 test("poller skipped height: "+mode,async()=>{
  const dataDir=mkdtempSync(join(tmpdir(),"indexer-skip-"));
  let head=parent;
  const source={
   fetchLastFinalizedHeight:async()=>head,
   fetchFinalizedBlock:async h=>{
    if(h===missing)throw Object.assign(new Error("missing"),{code:mode==="rate-limit"?"RATE_LIMIT":"BLOCK_NULL",retryable:true});
    const h1=h===parent?p:h===successor?(mode==="wrong-payload"?{...n,hash:"wrong"}:n):after;
    return {block:{header:h1},shards:[]};
   },
   verifyArchivalBlockAvailability:async h=>h===missing?{available:false,status:mode==="unverified"?"unverified":"unknown_block"}:{available:true,status:"available",blockHeader:mode==="bad-parent"?{...n,prev_hash:"wrong"}:n}
  };
  const make=()=>createPerpsIndexerService({dataDir,marketId:ARABICA_MARKET_ID,backfillTransactions:[],accountHistoryBackfillEnabled:false,blockPacingMs:0},{blockSource:source});
  let svc=make();
  try{
   await svc.start({startHttp:false,startPolling:false});
   assert.equal(svc.store.getCursor().indexedThroughHeight,parent);
   head=successor+1;
   await svc.pollOnce();
   const c=svc.store.getCursor();
   assert.equal(c.indexedThroughHeight,mode==="valid"?successor+1:parent);
   assert.equal(c.verifiedSkippedRanges?.length??0,mode==="valid"?1:0);
   await svc.stop();
   svc=make();
   await svc.start({startHttp:false,startPolling:false,skipInitialPoll:true});
   assert.equal(svc.store.getCursor().indexedThroughHeight,c.indexedThroughHeight);
   assert.deepEqual(svc.store.getCursor().verifiedSkippedRanges??[],c.verifiedSkippedRanges??[]);
  }finally{await svc.stop();rmSync(dataDir,{recursive:true,force:true});}
 });
}

test("poller advances across recurring and consecutive verified omissions", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "indexer-recurring-skip-"));
  const present = [parent, parent + 2, parent + 3, parent + 6, parent + 7];
  const blocks = new Map(present.map((h, i) => [h, header(h, present[i-1] ?? h-1, "block-"+h, "block-"+(present[i-1] ?? h-1))]));
  let head = parent;
  const source = {
    fetchLastFinalizedHeight: async () => head,
    fetchFinalizedBlock: async h => {
      if (!blocks.has(h)) throw Object.assign(new Error("missing"), {code:"BLOCK_NULL", retryable:true});
      return {block:{header:blocks.get(h)},shards:[]};
    },
    verifyArchivalBlockAvailability: async h => blocks.has(h)
      ? {available:true,status:"available",blockHeader:blocks.get(h)}
      : {available:false,status:"unknown_block"},
  };
  const make = () => createPerpsIndexerService({dataDir,marketId:ARABICA_MARKET_ID,backfillTransactions:[],accountHistoryBackfillEnabled:false,blockPacingMs:0},{blockSource:source});
  let svc = make();
  try {
    await svc.start({startHttp:false,startPolling:false});
    const initial = svc.store.getCursor();
    head = present.at(-1);
    await svc.pollOnce();
    const c = svc.store.getCursor();
    assert.equal(c.indexedThroughHeight, head);
    assert.equal(c.blocksProcessed, initial.blocksProcessed + present.length - 1);
    assert.deepEqual(c.verifiedSkippedRanges.map(r=>[r.fromHeight,r.toHeight]), [[parent+1,parent+1],[parent+4,parent+5]]);
    await svc.stop();
    svc = make();
    await svc.start({startHttp:false,startPolling:false,skipInitialPoll:true});
    assert.deepEqual(svc.store.getCursor().verifiedSkippedRanges, c.verifiedSkippedRanges);
  } finally { await svc.stop(); rmSync(dataDir,{recursive:true,force:true}); }
});
