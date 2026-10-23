// Generates the Tiptap fixtures of tests/fixtures/markdown/tiptap: the
// document a Tiptap 3 editor holds (`editor.getJSON()`) and the HTML it
// writes for it (`getHTML()`), from one source through Tiptap's own schema.
// Usage (in a scratch directory, so the packages stay out of the repo):
//
//   npm install --no-save @tiptap/html@3 @tiptap/starter-kit@3 \
//     @tiptap/extension-text-align@3 @tiptap/extension-text-style@3 \
//     @tiptap/extension-highlight@3 @tiptap/extension-table@3 \
//     @tiptap/extension-image@3 @tiptap/extension-subscript@3 \
//     @tiptap/extension-superscript@3 @tiptap/extension-list@3 \
//     @tiptap/extension-mathematics@3 @tiptap/extension-details@3 \
//     @tiptap/extension-mention@3 @tiptap/extension-youtube@3 \
//     @tiptap/extension-emoji@3
//   node generate.mjs <output directory>
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { generateHTML, generateJSON } from "@tiptap/html/server";
import StarterKit from "@tiptap/starter-kit";
import { TextAlign } from "@tiptap/extension-text-align";
import { TextStyleKit } from "@tiptap/extension-text-style";
import { Highlight } from "@tiptap/extension-highlight";
import { TableKit } from "@tiptap/extension-table";
import { Image } from "@tiptap/extension-image";
import { Subscript } from "@tiptap/extension-subscript";
import { Superscript } from "@tiptap/extension-superscript";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Mathematics } from "@tiptap/extension-mathematics";
import { Details, DetailsContent, DetailsSummary } from "@tiptap/extension-details";
import { Mention } from "@tiptap/extension-mention";
import { Youtube } from "@tiptap/extension-youtube";
import { Emoji } from "@tiptap/extension-emoji";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname } from "node:path";

const extensions = [
  StarterKit,
  TextAlign.configure({ types: ["heading", "paragraph", "image"] }),
  TextStyleKit,
  Highlight.configure({ multicolor: true }),
  TableKit.configure({ table: { resizable: true } }),
  Image,
  Subscript,
  Superscript,
  TaskList,
  TaskItem.configure({ nested: true }),
  Mathematics,
  Details,
  DetailsSummary,
  DetailsContent,
  Mention,
  Youtube,
  Emoji,
];

// Every construct both an editor's HTML and its JSON carry the same way:
// docrs renders the two to the same document.
const core = [
  '<h2 style="text-align: center">Advice</h2>',
  '<p>Plain <strong>bold</strong> <em>italic</em> <u>underlined</u> <s>struck</s> <code>code</code> ',
  '<sub>sub</sub> <sup>sup</sup> <a href="https://example.org/advice" title="Advice">a <strong>bold</strong> link</a> ',
  '<span style="color: #dc2626">red</span> <span style="font-family: &quot;Courier New&quot;, monospace">mono</span> ',
  '<span style="font-size: 18px">large</span> <span style="background-color: #bbf7d0">shaded</span> ',
  '<mark>yellow</mark> <mark data-color="#fef08a" style="background-color: #fef08a">pale</mark>.</p>',
  '<p style="text-align: right">Right</p><p style="text-align: justify">Justified</p><p></p>',
  '<ul><li><p>one</p><ul><li><p>nested</p></li></ul></li><li><p>two</p></li></ul>',
  '<ol start="3"><li><p>third</p></li><li><p>fourth</p></li></ol>',
  '<ul data-type="taskList"><li data-type="taskItem" data-checked="true"><p>done</p></li>',
  '<li data-type="taskItem" data-checked="false"><p>open</p></li></ul>',
  '<blockquote><p>quoted</p></blockquote>',
  '<pre><code class="language-rust">fn main() {}\n// two</code></pre><hr>',
  '<table style="width: 520px"><colgroup><col style="width: 240px"><col style="width: 140px"><col style="width: 140px"></colgroup><tbody>',
  '<tr><th colwidth="240"><p>Product</p></th><th colspan="2" colwidth="140,140"><p>Both</p></th></tr>',
  '<tr><td rowspan="2" colwidth="240"><p>Two rows</p></td><td colwidth="140"><p style="text-align: right">$1</p></td><td colwidth="140"><p>x</p></td></tr>',
  '<tr><td colwidth="140"><p>y</p></td><td colwidth="140"><p>z</p></td></tr></tbody></table>',
  '<img src="https://store.example/logo.png?token=t" alt="Logo" title="The logo" width="200" height="80">',
  '<img src="https://store.example/seal.png?token=s" alt="Seal" width="120" height="120" style="text-align: center">',
  '<p>a line<br>break</p>',
  '<table><colgroup><col style="width: 120px"><col><col></colgroup><tbody>',
  '<tr><th colwidth="120"><p>Asset</p></th><th><p>Description</p></th><th><p>Value</p></th></tr>',
  '<tr><td colwidth="120"><p>Home</p></td><td><p>Family home, owned jointly</p></td><td><p>$1,200,000</p></td></tr></tbody></table>',
  "<h2>After</h2>",
].join("");

// Nodes whose HTML is a placeholder for the editor's own rendering (math,
// details, mentions, emoji, embeds): the JSON carries what the HTML does not.
const extras = [
  '<p>See <span data-type="mention" data-id="42" data-label="Jane"></span> and ',
  '<span data-type="emoji" data-name="rocket"></span> for <span data-type="inline-math" data-latex="E = mc^2"></span>.</p>',
  '<div data-type="block-math" data-latex="\\int_0^1 x\\,dx"></div>',
  '<details><summary>More</summary><div data-type="detailsContent"><p>hidden</p></div></details>',
  '<div data-youtube-video><iframe src="https://www.youtube.com/watch?v=dQw4w9WgXcQ"></iframe></div>',
].join("");

const out = process.argv[2];
if (!out) {
  console.error("usage: node generate.mjs <output directory>");
  process.exit(2);
}
mkdirSync(out, { recursive: true });
for (const [name, source] of [["core", core], ["extras", extras]]) {
  const json = generateJSON(source, extensions);
  writeFileSync(join(out, `${name}.json`), JSON.stringify(json, null, 1) + "\n");
  writeFileSync(join(out, `${name}.html`), generateHTML(json, extensions) + "\n");
}
let dir = dirname(fileURLToPath(import.meta.resolve("@tiptap/core")));
while (!dir.endsWith("core")) dir = dirname(dir);
const version = JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).version;
console.log(`wrote core.{json,html} and extras.{json,html} with @tiptap/core ${version}`);
