// Entry for build.mjs: the formatted view and the Edit sheet, as window.pfEditor.
import { mount, parse, sourceBlocks, serializer, schema } from './editor.js';
import { openSheet } from './sheet.js';
import { plain } from './plain.js';

window.pfEditor = { mount, parse, sourceBlocks, serializer, schema, openSheet, plain };
