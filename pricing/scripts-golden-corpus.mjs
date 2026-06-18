import { SuiJsonRpcClient, getJsonRpcFullnodeUrl } from '@mysten/sui/jsonRpc';
import { Transaction } from '@mysten/sui/transactions';
import { bcs } from '@mysten/sui/bcs';
import { readFileSync, writeFileSync } from 'node:fs';

const PKG='0xf5ea2b3749c65d6e56507cc35388719aadb28f9cab873696a2f8687f5c785138';
const SENDER='0x1509b5fdf09296b2cf749a710e36da06f5693ccd5b2144ad643b3a895abcbc4c';
const c=new SuiJsonRpcClient({url:getJsonRpcFullnodeUrl('testnet')});
const oids=JSON.parse(readFileSync('/tmp/oids.json','utf8'));
// strike grid as relative pct of forward
const PCTS=[-5,-3,-2,-1,-0.5,-0.25,0,0.25,0.5,1,2,3,5];
const u64=v=>bcs.u64().parse(Uint8Array.from(v));

const clk=await c.getObject({id:'0x6',options:{showContent:true}});
const clockNow=clk.data.content.fields.timestamp_ms;

const corpus=[];
for(const oid of oids){
  const o=await c.getObject({id:oid,options:{showContent:true}});
  const f=o.data.content.fields;
  const fwd=BigInt(f.prices.fields.forward);
  const grid=PCTS.map(p=>({pct:p, K:(fwd*BigInt(Math.round((1+p/100)*1e6))/1_000_000n)}));
  const tx=new Transaction();
  for(const g of grid){
    tx.moveCall({target:`${PKG}::oracle::binary_price_pair`,arguments:[tx.object(oid),tx.pure.u64(g.K),tx.object('0x6')]});
    tx.moveCall({target:`${PKG}::oracle::compute_price`,arguments:[tx.object(oid),tx.pure.u64(g.K)]});
  }
  let vectors=[]; let err=null;
  try{
    const r=await c.devInspectTransactionBlock({sender:SENDER,transactionBlock:tx});
    if(r.error){err=r.error;}
    else{
      let i=0;
      for(const g of grid){
        const rv=r.results[i].returnValues;
        const up=u64(rv[0][0]).toString(), dn=u64(rv[1][0]).toString();
        const cp=u64(r.results[i+1].returnValues[0][0]).toString();
        vectors.push({pct:g.pct,strike:g.K.toString(),up,down:dn,compute_price:cp});
        i+=2;
      }
    }
  }catch(e){err=String(e.message||e);}
  corpus.push({
    oracle_id:oid, underlying:f.underlying_asset, expiry_ms:f.expiry, active:f.active,
    settlement_price:f.settlement_price,
    spot:f.prices.fields.spot, forward:f.prices.fields.forward,
    svi:{a:f.svi.fields.a,b:f.svi.fields.b,
      rho:(f.svi.fields.rho.fields.is_negative?'-':'')+f.svi.fields.rho.fields.magnitude,
      m:(f.svi.fields.m.fields.is_negative?'-':'')+f.svi.fields.m.fields.magnitude,
      sigma:f.svi.fields.sigma},
    err, vectors
  });
  process.stderr.write(`${f.underlying_asset} exp=${f.expiry} active=${f.active} -> ${err?'ERR '+err:vectors.length+' pts'}\n`);
}
// parity check across all
let bad=0,tot=0;
for(const o of corpus) for(const v of o.vectors){tot++; if((BigInt(v.up)+BigInt(v.down)).toString()!=='1000000000')bad++;}
const out={_comment:'Golden corpus: all live DeepBook Predict oracles, strike grid devInspect. prices/strikes/spot/forward + svi a/b/sigma scaled 1e9; rho/m signed 1e9.',
  source:{network:'testnet',package:PKG,captured_clock_ms:clockNow,strike_grid_pct_of_forward:PCTS,method:'devInspect oracle::binary_price_pair + compute_price'},
  parity:{total_points:tot,up_plus_down_neq_1e9:bad},
  oracles:corpus};
writeFileSync('/tmp/gv/corpus.json',JSON.stringify(out,null,1));
console.log(`DONE oracles=${corpus.length} points=${tot} parity_violations=${bad} errored=${corpus.filter(o=>o.err).length}`);
