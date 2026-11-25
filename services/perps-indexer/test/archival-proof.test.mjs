import test from "node:test";
import assert from "node:assert/strict";
import { createBlockSource } from "../src/block-source.mjs";
const endpoint = "https://archival-rpc.testnet.near.org";
const config = {nearDataBaseUrl:"https://testnet.neardata.xyz",rpcEndpoints:[endpoint],archivalRpcEndpoints:[endpoint]};
const source = (fetchFn, overrides={}) => createBlockSource({...config,...overrides},{fetchFn});
const json = (body,status=200) => new Response(JSON.stringify(body),{status,headers:{"content-type":"application/json"}});
test("explicit UNKNOWN_BLOCK is distinguished from network failures",async()=>{
 const s=source(async()=>json({error:{cause:{name:"UNKNOWN_BLOCK"},message:"DB Not Found"}}));
 assert.equal((await s.verifyArchivalBlockAvailability(277836283)).status,"unknown_block");
});
test("rate limits, server errors and generic RPC errors are inconclusive",async()=>{
 for(const fn of [async()=>json({},429),async()=>json({},503),async()=>json({error:{message:"internal error"}}),async()=>{throw Error("offline");}]){
  assert.equal((await source(fn).verifyArchivalBlockAvailability(277836283)).status,"unverified");
 }
});
test("no archival endpoint cannot establish absence",async()=>{
 assert.equal((await source(async()=>{throw Error("unexpected request");},{archivalRpcEndpoints:[]}).verifyArchivalBlockAvailability(277836283)).status,"unverified");
});
test("successor header retains parent continuity evidence",async()=>{
 const header={height:277836284,prev_height:277836282,hash:"successor",prev_hash:"parent"};
 const result=await source(async()=>json({result:{header}})).verifyArchivalBlockAvailability(header.height);
 assert.equal(result.status,"available");
 assert.deepEqual(result.blockHeader,header);
});
