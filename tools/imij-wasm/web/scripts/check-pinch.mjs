// Trusted multi-touch events through Chromium's input protocol, not synthetic DOM events.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);if(!out)throw Error('usage: check-pinch WEB URL FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch();
try{
 const context=await browser.newContext({viewport:{width:1440,height:1050},hasTouch:true});const page=await context.newPage();await page.goto(url);
 async function settled(id=-1){await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});const error=await page.evaluate(()=>window.demoError);if(error)throw Error(error);return page.evaluate(()=>window.demoState);}
 await settled();let state=await page.evaluate(()=>window.demoState);await page.selectOption('#margin','0');await settled(state.requestId);state=await page.evaluate(()=>window.demoState);await page.selectOption('#workers','1');const before=await settled(state.requestId);
 const box=await page.locator('#view').boundingBox(),cx=box.x+box.width/2,cy=box.y+box.height/2;
 const dy=20*box.height/await page.locator('#view').evaluate(c=>c.height),panDy=30*box.height/await page.locator('#view').evaluate(c=>c.height);
 const client=await context.newCDPSession(page);await client.send('Emulation.setTouchEmulationEnabled',{enabled:true,maxTouchPoints:2});
 const point=(id,x,y)=>({id,x,y,radiusX:8,radiusY:8,force:1});
 await client.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[point(0,cx-100,cy),point(1,cx+100,cy)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[point(0,cx-20,cy+dy),point(1,cx+80,cy+dy)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});const pinched=await settled(before.requestId);
 if(Math.abs(pinched.view.scale-before.view.scale*.5)>1e-6||pinched.representation.level!=='half')throw Error('trusted pinch did not select half level');
 if(Math.abs(pinched.view.x-(before.view.x-30/pinched.view.scale))>.01||Math.abs(pinched.view.y-(before.view.y-20/pinched.view.scale))>.01)throw Error('trusted pinch anchor');
 await client.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[point(2,cx,cy)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[point(2,cx+50,cy+panDy)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});const panned=await settled(pinched.requestId);
 if(Math.abs(panned.view.scale-pinched.view.scale)>1e-6||Math.abs(panned.view.x-(pinched.view.x-50/panned.view.scale))>.01||Math.abs(panned.view.y-(pinched.view.y-30/panned.view.scale))>.01)throw Error('one-finger pan after pinch');
 writeFileSync(resolve(out,'results.json'),JSON.stringify({version:browser.version(),trustedTouch:true,before,pinched,panned},null,2));await page.screenshot({path:resolve(out,'pinched.png'),fullPage:true});console.log('Trusted pinch, half-level switch, midpoint anchor and subsequent one-finger pan PASS');
}finally{await browser.close();}
