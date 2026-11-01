// Diagnostic OCR using the project's existing Tesseract.js dependency.
// Recognition is local. Only public language weights are downloaded.
const fs = require('node:fs');
const path = require('node:path');
const {createWorker, PSM} = require('tesseract.js');
(async () => {
  const manifestMode = process.argv[2] === '--manifest';
  const input = path.resolve(process.argv[manifestMode ? 3 : 2]);
  const output = path.resolve(process.argv[manifestMode ? 4 : 3]);
  const cachePath = path.resolve('tmp/pdfs/local-ocr-models');
  fs.mkdirSync(cachePath,{recursive:true});
  const worker = await createWorker('chi_sim',1,{cachePath,langPath:'https://tessdata.projectnaptha.com/4.0.0'});
  try {
    if (manifestMode) {
      await worker.setParameters({tessedit_pageseg_mode:PSM.SINGLE_BLOCK,preserve_interword_spaces:'1'});
      const results={};
      for (const entry of JSON.parse(fs.readFileSync(input,'utf8'))) {
        const {data} = await worker.recognize(entry.path);
        results[entry.id]={...entry,lines:(data.lines||[]).map(l=>({text:l.text,bbox:l.bbox,confidence:l.confidence,words:(l.words||[]).map(w=>({text:w.text,bbox:w.bbox,confidence:w.confidence}))}))};
      }
      fs.writeFileSync(output,JSON.stringify(results,null,2));
      console.log(JSON.stringify({patches:Object.keys(results).length}));
      return;
    }
    await worker.setParameters({tessedit_pageseg_mode:PSM.SPARSE_TEXT,preserve_interword_spaces:'1'});
    const {data} = await worker.recognize(input);
    const lines = data.lines || (data.blocks||[]).flatMap(b=>(b.paragraphs||[]).flatMap(p=>p.lines||[]));
    fs.writeFileSync(output,JSON.stringify({confidence:data.confidence,lines:lines.map(l=>({text:l.text,bbox:l.bbox,confidence:l.confidence,words:(l.words||[]).map(w=>({text:w.text,bbox:w.bbox,confidence:w.confidence}))}))},null,2));
    console.log(JSON.stringify({lines:lines.length,confidence:data.confidence}));
  } finally { await worker.terminate(); }
})().catch(error=>{console.error(String(error));process.exitCode=1;});
