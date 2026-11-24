import test from 'node:test';import assert from 'node:assert/strict';
import {gallerySelection} from '../gigapixel/gallery.js';
const scenes=[{id:'colca',title:'Vallée de Colca',credit:'Martin St-Amant',sourceUrl:'https://commons.wikimedia.org/wiki/colca',width:30000,height:11000,tags:['mountains']},{id:'webb',title:'Cosmic Cliffs',credit:'NASA',sourceUrl:'https://svs.gsfc.nasa.gov/31186',width:14575,height:8441,collection:'Astronomy'},{id:'vista',title:'Milky Way',credit:'ESO',sourceUrl:'https://www.eso.org/public/images/eso1242a/',width:108199,height:81503,tags:['Astronomy']}];
test('gallery search combines normalized title, credit, tags and collection',()=>{
 assert.deepEqual(gallerySelection(scenes,{query:'vallee martin mountains'}).map(s=>s.id),['colca']);
 assert.deepEqual(gallerySelection(scenes,{query:'astronomy'}).map(s=>s.id),['webb','vista']);
 assert.deepEqual(gallerySelection(scenes,{query:'no match'}),[]);
});
test('source and sorting compose without mutating the catalog',()=>{
 assert.deepEqual(gallerySelection(scenes,{source:'svs.gsfc.nasa.gov',sort:'largest'}).map(s=>s.id),['webb']);
 assert.deepEqual(gallerySelection(scenes,{sort:'largest'}).map(s=>s.id),['vista','colca','webb']);
 assert.deepEqual(gallerySelection(scenes,{sort:'title'}).map(s=>s.id),['webb','vista','colca']);
 assert.deepEqual(scenes.map(s=>s.id),['colca','webb','vista']);
});
