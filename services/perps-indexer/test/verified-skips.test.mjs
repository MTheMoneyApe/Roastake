import test from "node:test";
import assert from "node:assert/strict";
import { buildCoverageSummary } from "../src/coverage.mjs";

const proof = {fromHeight:277836283,toHeight:277836283,parentHeight:277836282,parentHash:"AkkEPVov4ZmFpuajEQp4npjjj3eNn2NYHxsFZcAgfQGN",successorHeight:277836284,successorHash:"8Ep7eCe4PSdp5s6LuaimSrSBjFcLPyd3g36rg12aA3tv"};
const cursor = {coverageStartHeight:277802105,indexedThroughHeight:277836284,backfillBlockRange:{minHeight:277160696,maxHeight:277174472},scannedSegments:[{startHeight:277802105,endHeight:277836282},{startHeight:277836284,endHeight:277836284}]};
test("verified omission does not count as scanned or hide historical gap",()=>{
 const s=buildCoverageSummary({cursor:{...cursor,verifiedSkippedRanges:[proof]}});
 assert.equal(s.totalGapBlocks,627632);
 assert.equal(s.totalScannedBlocks,277836282-277802105+2);
 assert.equal(s.verifiedSkippedRanges.length,1);
 assert.equal(s.coverageComplete,false);
});
test("unproven omission remains a gap",()=>{
 const s=buildCoverageSummary({cursor});
 assert.equal(s.totalGapBlocks,627633);
});
test("malformed proof cannot close gap",()=>{
 for(const p of [{...proof,parentHash:""},{...proof,successorHeight:277836285},{...proof,parentHeight:277836281}]){
  assert.equal(buildCoverageSummary({cursor:{...cursor,verifiedSkippedRanges:[p]}}).totalGapBlocks,627633);
 }
});
test("multiple absent heights remain distinct from scanned blocks",()=>{
 const c={coverageStartHeight:10,indexedThroughHeight:14,scannedSegments:[{startHeight:10,endHeight:10},{startHeight:14,endHeight:14}],verifiedSkippedRanges:[{fromHeight:11,toHeight:13,parentHeight:10,parentHash:"parent",successorHeight:14,successorHash:"successor"}]};
 const s=buildCoverageSummary({cursor:c});
 assert.equal(s.totalScannedBlocks,2);
 assert.equal(s.totalGapBlocks,0);
 assert.equal(s.coverageComplete,true);
});
