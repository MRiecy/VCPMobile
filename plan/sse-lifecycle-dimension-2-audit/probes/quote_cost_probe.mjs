// Audit-only host probe. Run from the repository root:
// node plan/sse-lifecycle-dimension-2-audit/probes/quote_cost_probe.mjs
// Requires the current Rust toolchain and existing compatible debug dependency artifacts.
// Reads the production AST and extracts the production quote functions verbatim;
// it does not reimplement the algorithm or modify production files.
// The legacy suffix-check column describes the audited pre-fix algorithm only;
// timings always come from the current working-tree implementation.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const root = process.cwd();
const parserPath = 'src-tauri/src/vcp_modules/chat/pre_renderer/markdown_parser.rs';
const astPath = 'src-tauri/src/vcp_modules/chat/pre_renderer/markdown_ast.rs';
const parser = fs.readFileSync(parserPath, 'utf8');
const start = parser.indexOf('fn split_text_by_quotes(');
const end = parser.indexOf('fn apply_quote_merging(', start);
if (start < 0 || end < 0) throw new Error('Production quote function boundaries changed');
const functions = parser.slice(start, end);
const nesting = parser.match(/const MAX_MARKDOWN_NESTING_DEPTH: usize = \d+;/)?.[0];
if (!nesting) throw new Error('Production nesting constant not found');
const deps = path.resolve('src-tauri/target/debug/deps');
function dependency(name) {
  const candidates = fs.readdirSync(deps)
    .filter(file => file.startsWith(`lib${name}-`) && file.endsWith('.rlib'))
    .map(file => ({ file: path.join(deps, file), time: fs.statSync(path.join(deps, file)).mtimeMs }))
    .sort((a, b) => b.time - a.time);
  if (!candidates.length) throw new Error(`Build the existing host dependencies first: ${name}`);
  return candidates[0].file;
}
const outputDir = path.resolve('src-tauri/target/sse-audit-quote-probe');
fs.mkdirSync(outputDir, { recursive: true });
const source = `#![allow(dead_code, unused_imports)]
#[path = ${JSON.stringify(path.resolve(astPath).replaceAll('\\', '/'))}]
mod markdown_ast;
use markdown_ast::InlineNode;
${nesting}
${functions}
fn main() {
    println!("kind,characters,utf8_bytes,legacy_suffix_checks,median_us");
    for n in [1000usize, 2000, 4000, 8000, 16000, 21845] {
        for (kind, character) in [("plain", "a"), ("unclosed_cjk_openers", "“")] {
            let input = character.repeat(n);
            let mut samples = Vec::new();
            for _ in 0..5 {
                let nodes = vec![InlineNode::text(input.clone())];
                let started = std::time::Instant::now();
                let output = merge_quote_nodes(split_text_by_quotes(std::hint::black_box(nodes)));
                let elapsed = started.elapsed().as_micros();
                let restored: String = output.iter().map(|node| match node {
                    InlineNode::Text { value } => value.as_str(),
                    _ => panic!("Unexpected quote wrapper for unmatched input"),
                }).collect();
                assert_eq!(restored, input);
                std::hint::black_box(output);
                samples.push(elapsed);
            }
            samples.sort_unstable();
            let checks = if kind == "plain" { 0 } else { n * (n + 1) / 2 };
            println!("{},{},{},{},{}", kind, n, input.len(), checks, samples[2]);
        }
    }
}
`;
const sourcePath = path.join(outputDir, 'quote_probe.rs');
const executable = path.join(outputDir, process.platform === 'win32' ? 'quote_probe.exe' : 'quote_probe');
fs.writeFileSync(sourcePath, source, 'utf8');
console.log(JSON.stringify({
  head: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  toolchain: execFileSync('rustc', ['--version'], { encoding: 'utf8' }).trim(),
  parser: parserPath,
  parserSha256: createHash('sha256').update(parser).digest('hex'),
  hostOnly: true,
  optimization: 'rustc -C opt-level=3; cached debug serde/rustc_hash dependencies',
}));
execFileSync('rustc', [sourcePath, '--edition=2021', '-C', 'opt-level=3', '-L', `dependency=${deps}`,
  '--extern', `serde=${dependency('serde')}`, '--extern', `rustc_hash=${dependency('rustc_hash')}`,
  '-o', executable], { stdio: 'inherit', cwd: root });
process.stdout.write(execFileSync(executable, [], { encoding: 'utf8', timeout: 60000 }));
