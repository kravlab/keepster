/*
 * PROTOTYPE (ticket 09) — measurement entry: Monaco assembled the way CodeMirror is, i.e. the editor
 * core plus exactly the curated 12 languages, instead of the package's default entry which also
 * registers ~80 grammars and an LSP client. This is the fair counterpart to the CodeMirror build.
 */
import * as monaco from 'monaco-editor/editor/editor.api'
import 'monaco-editor/languages/definitions/rust/register'
import 'monaco-editor/languages/definitions/typescript/register'
import 'monaco-editor/languages/definitions/javascript/register'
// JSON has no Monarch grammar in monaco 0.56: highlighting it means pulling the JSON language *service*.
import 'monaco-editor/languages/features/json/register'
import 'monaco-editor/languages/definitions/ini/register'
import 'monaco-editor/languages/definitions/yaml/register'
import 'monaco-editor/languages/definitions/markdown/register'
import 'monaco-editor/languages/definitions/python/register'
import 'monaco-editor/languages/definitions/go/register'
import 'monaco-editor/languages/definitions/sql/register'
import 'monaco-editor/languages/definitions/shell/register'
import 'monaco-editor/languages/definitions/html/register'
import 'monaco-editor/languages/definitions/css/register'

;(globalThis as Record<string, unknown>).__keepster_monaco_core = monaco.editor.createDiffEditor
