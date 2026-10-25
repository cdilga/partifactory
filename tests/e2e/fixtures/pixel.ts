import * as fs from 'node:fs';
import * as path from 'node:path';

const root=path.resolve(__dirname,'../../..');
export const pixelFixture=fs.readFileSync(path.join(__dirname,'pixel.html'),'utf8').trimEnd();
export function contractPixel() {
  const document=fs.readFileSync(path.join(root,'docs/agent-contract.md'),'utf8');
  const source=document.split('Hermes writes `board.html`:')[1].split('```html\n')[1].split('\n```')[0];
  const edit=JSON.parse(document.split('**4. Fix with one edit**')[1].split('```json\n')[1].split('\n→')[0]).edits[0];
  if(source.split(edit.old_str).length!==2)throw new Error('Contract edit must match exactly once');
  return {source,edit,fixed:source.replace(edit.old_str,edit.new_str)};
}
