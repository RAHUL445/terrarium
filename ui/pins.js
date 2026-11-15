// Device-local, per-space pins. Labels never serve as identity.
(function(root){
  'use strict';
  const valid = p => p && ['page','app','folder'].includes(p.kind) && typeof p.id === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(p.id);
  const key = p => `${p.kind}:${p.id}`;
  function normalize(value) {
    const pins=[], seen=new Set();
    for(const p of Array.isArray(value?.pins)?value.pins:[]) if(valid(p)&&!seen.has(key(p))&&pins.length<100){seen.add(key(p));pins.push({kind:p.kind,id:p.id});}
    return {version:1,pins};
  }
  function read(storage,space){
    try {
      const raw=storage.getItem(`pp-pins:${space}`);
      if(raw!==null) return normalize(JSON.parse(raw));
      const legacy=JSON.parse(storage.getItem(`pp-layout:${space}`)||'null');
      const migrated=normalize({pins:Array.isArray(legacy?.nav)?legacy.nav.map(id=>({kind:'app',id})):[]});
      write(storage,space,migrated); return migrated;
    }catch(_){return normalize(null);}
  }
  function write(storage,space,state){try{storage.setItem(`pp-pins:${space}`,JSON.stringify(normalize(state)));return true;}catch(_){return false;}}
  function toggle(state,pin){const clean=normalize(state);if(!valid(pin))return clean;return normalize({pins:clean.pins.some(p=>key(p)===key(pin))?clean.pins.filter(p=>key(p)!==key(pin)):[...clean.pins,pin]});}
  function move(state,pin,delta){const clean=normalize(state),i=clean.pins.findIndex(p=>key(p)===key(pin)),to=i+delta;if(i>=0&&to>=0&&to<clean.pins.length)[clean.pins[i],clean.pins[to]]=[clean.pins[to],clean.pins[i]];return clean;}
  const api={key,normalize,read,write,toggle,move};
  if(typeof module!=='undefined'&&module.exports)module.exports=api;else root.TerrariumPins=api;
})(typeof window==='undefined'?globalThis:window);
