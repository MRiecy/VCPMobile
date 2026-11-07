/**
 * T3 源码模式纯函数单测（bridge 返回归一化 / PTY 输出解析 / ReadCode·Outline·FindSymbol·Trace 解析 /
 * 路径与文本工具）。这些函数直接决定「FileOperator 返回结构不确定」时的兼容性，单独覆盖。
 */
import { describe, expect, it } from 'vitest';
import {
  countContentLines,
  ensureRelativePath,
  extractMarkedOutput,
  extractReadCodeBody,
  extractReviewWarnings,
  flattenTreeRows,
  formatSourceSize,
  joinProjectPath,
  languageLabel,
  normalizeDirectoryPayload,
  normalizeRelativePath,
  normalizeSearchPayload,
  parseFindSymbolEnvelope,
  parseFindSymbolText,
  parseOutlineEnvelope,
  parseOutlineText,
  parsePlainDirectoryText,
  parsePtyFindOutput,
  parseReadCodeEnvelope,
  parseTraceEnvelope,
  sameFileText,
  shellQuote,
  splitSearchTokens,
  toShellPath,
  unwrapFilePayload,
} from '../../../../features/projectforge/useProjectForgeSource';
import {
  findSymbolEnvelope,
  outlineEnvelope,
  readEnvelope,
  textEnvelope,
  traceEnvelope,
} from './sourceTestUtils';

const ROOT = '/home/user/proj/demo';

describe('目录列表返回归一化（FileOperator 结构未知）', () => {
  it('对象数组：识别目录/文件与大小', () => {
    const entries = normalizeDirectoryPayload(
      [
        { name: 'src', isDirectory: true },
        { name: 'README.md', type: 'file', size: 128 },
      ],
      '',
      ROOT,
    );
    expect(entries?.map((entry) => [entry.name, entry.type, entry.size])).toEqual([
      ['src', 'dir', undefined],
      ['README.md', 'file', 128],
    ]);
  });

  it('分组对象 {dirs, files}：合并并按目录优先排序', () => {
    const entries = normalizeDirectoryPayload(
      { dirs: [{ name: 'lib' }], files: [{ name: 'b.js' }, { name: 'a.js' }] },
      '',
      ROOT,
    );
    expect(entries?.map((entry) => entry.name)).toEqual(['lib', 'a.js', 'b.js']);
  });

  it('字符串数组（插件只回路径）：取 basename 并拼相对路径', () => {
    const entries = normalizeDirectoryPayload([`${ROOT}/src/a.ts`, 'b.md'], 'src', ROOT);
    expect(entries?.map((entry) => entry.path)).toEqual(['src/a.ts', 'src/b.md']);
  });

  it('JSON 字符串与 {details:{entries}} / {data:[...]} 包装都能解', () => {
    const jsonText = JSON.stringify([{ name: 'a.txt', type: 'file' }]);
    expect(normalizeDirectoryPayload(jsonText, '', ROOT)?.map((entry) => entry.path)).toEqual(['a.txt']);
    expect(
      normalizeDirectoryPayload({ details: { entries: [{ name: 'b.txt', type: 'file' }] } }, '', ROOT)?.map(
        (entry) => entry.path,
      ),
    ).toEqual(['b.txt']);
    expect(normalizeDirectoryPayload({ data: [{ name: 'c.txt', type: 'file' }] }, '', ROOT)?.map((e) => e.path)).toEqual([
      'c.txt',
    ]);
  });

  it('嵌套 children 标记目录；隐藏文件在 filter 前由调用方处理', () => {
    const entries = normalizeDirectoryPayload(
      [{ name: 'nested', children: [{ name: 'x.js', type: 'file' }] }],
      'sub',
      ROOT,
    );
    expect(entries).toEqual([{ name: 'nested', path: 'sub/nested', type: 'dir' }]);
  });

  it('纯文本列表（ls -la / 图标列表）可解析，错误文本返回 null', () => {
    const ls = '-rw-r--r-- 1 root root 120 Nov  7 10:00 README.md\ndrwxr-xr-x 2 root root 4096 Nov  7 10:00 src';
    expect(parsePlainDirectoryText(ls, '', ROOT)?.map((entry) => [entry.name, entry.type])).toEqual([
      ['README.md', 'file'],
      ['src', 'dir'],
    ]);
    expect(normalizeDirectoryPayload(ls, '', ROOT)?.map((entry) => entry.name)).toEqual(['src', 'README.md']);
    expect(normalizeDirectoryPayload('📁 src\n📄 README.md', '', ROOT)?.map((entry) => entry.path)).toEqual([
      'src',
      'README.md',
    ]);
    expect(normalizeDirectoryPayload('错误：目录不存在', '', ROOT)).toBeNull();
    expect(normalizeDirectoryPayload({ unexpected: true }, '', ROOT)).toBeNull();
  });

  it('unwrapFilePayload 逐层剥离 result/details/content/data', () => {
    expect(unwrapFilePayload({ ok: true, result: { details: { entries: [] }, content: [] } })).toEqual({ entries: [] });
    expect(unwrapFilePayload({ content: [{ type: 'text', text: '["x"]' }] })).toEqual(['x']);
  });
});

describe('搜索结果归一化', () => {
  it('对象数组 / 字符串 / {files} 包装都能得到文件条目', () => {
    expect(normalizeSearchPayload([{ path: 'src/main.ts' }], ROOT)?.map((entry) => entry.name)).toEqual(['main.ts']);
    expect(normalizeSearchPayload('src/main.ts\n./lib/util.ts', ROOT)?.map((entry) => entry.path)).toEqual([
      'src/main.ts',
      'lib/util.ts',
    ]);
    expect(normalizeSearchPayload({ files: [{ path: `${ROOT}/a/b.ts` }] }, ROOT)?.map((entry) => entry.path)).toEqual([
      'a/b.ts',
    ]);
    expect(normalizeSearchPayload('Search failed: no such directory', ROOT)).toBeNull();
  });
});

describe('ReadCode envelope 解析', () => {
  it('剥离桌面文件头与 md 围栏，正文逐字节还原（CRLF 保留）', () => {
    const body = 'const a = 1;\r\nconst b = 2;\r\n';
    const parsed = parseReadCodeEnvelope(readEnvelope('src/a.ts', body));
    expect(parsed.rawText).toBe(body);
    expect(parsed.text).toBe('const a = 1;\nconst b = 2;\n');
    expect(parsed.eol).toBe('CRLF');
    expect(parsed.failed).toEqual([]);
    expect(parsed.size).toBe(body.length);
  });

  it('正文不含文件头「####」与「- 总行数」等包装行', () => {
    const body = '# README\n\n第一段\n第二段\n';
    const parsed = parseReadCodeEnvelope(readEnvelope('README.md', body));
    expect(parsed.rawText).toBe(body);
    expect(parsed.text).toBe(body);
    expect(parsed.text).not.toContain('####');
    expect(parsed.text).not.toContain('总行数');
    expect(parsed.text).not.toContain('```');
  });

  it('正文本身包含 ``` 围栏时按自适应围栏长度正确剥离', () => {
    const body = 'a\n```js\ncode\n```\nb\n';
    const parsed = parseReadCodeEnvelope(readEnvelope('doc.md', body));
    expect(parsed.rawText).toBe(body);
    expect(extractReadCodeBody(readEnvelope('doc.md', body).result.content[1].text)).toBe(body);
  });

  it('空文件正文为空串', () => {
    const parsed = parseReadCodeEnvelope(readEnvelope('empty.txt', ''));
    expect(parsed.rawText).toBe('');
    expect(parsed.text).toBe('');
  });

  it('空文件正文为空串，failed/skipped 归一为字符串数组', () => {
    const parsed = parseReadCodeEnvelope(
      textEnvelope(['已读取文件', ''], {
        failed: [{ path: 'src/x.ts', reason: '文件不存在' }],
        skipped: ['src/y.ts'],
        read: [],
      }),
    );
    expect(parsed.text).toBe('');
    expect(parsed.failed).toEqual(['src/x.ts：文件不存在']);
    expect(parsed.skipped).toEqual(['src/y.ts']);
  });

  it('图片等多模态 content 标记 multimodal', () => {
    const parsed = parseReadCodeEnvelope(textEnvelope(['截图'], {}));
    expect(parsed.multimodal).toBe(false);
    const multimodal = parseReadCodeEnvelope({
      ok: true,
      result: { content: [{ type: 'image_url', text: '' }], details: {} },
    });
    expect(multimodal.multimodal).toBe(true);
  });
});

describe('Outline / FindSymbol / Trace 解析', () => {
  it('Outline details.files[].symbols → 名称/种类/行号/层级', () => {
    const items = parseOutlineEnvelope(
      outlineEnvelope('src/a.ts', [
        { name: 'Foo', kind: 'class', start: 3, end: 20 },
        { name: 'bar', kind: 'method', start: 6, end: 9, depth: 1 },
      ]),
    );
    expect(items).toEqual([
      { name: 'Foo', kind: 'class', start: 3, end: 20, depth: 0 },
      { name: 'bar', kind: 'method', start: 6, end: 9, depth: 1 },
    ]);
  });

  it('Outline 纯文本（缩进 + L 起止 + 括号 kind）兜底', () => {
    const items = parseOutlineText('- Foo (class) L3-20\n  - bar (method) L6-9\n摘要行没有行号');
    expect(items).toEqual([
      { name: 'Foo', kind: 'class', start: 3, end: 20, depth: 0 },
      { name: 'bar', kind: 'method', start: 6, end: 9, depth: 1 },
    ]);
  });

  it('FindSymbol details.hits → 路径/限定名/行号/签名', () => {
    const parsed = parseFindSymbolEnvelope(
      findSymbolEnvelope([{ path: 'src/a.ts', name: 'Store.put', kind: 'method', start: 10, end: 20, signature: 'put()' }]),
    );
    expect(parsed.hits).toEqual([
      { path: 'src/a.ts', name: 'Store.put', kind: 'method', start: 10, end: 20, signature: 'put()', raw: 'src/a.ts#Store.put' },
    ]);
    expect(parsed.total).toBe(1);
    expect(parsed.note).toContain('扫描 3 个文件');
  });

  it('FindSymbol 纯文本（路径#限定名 · L起-止 · kind · 签名）兜底', () => {
    const hits = parseFindSymbolText('src/a.js#Store.putBlob · L10-20 · method · putBlob(v)');
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({
      path: 'src/a.js',
      name: 'Store.putBlob',
      kind: 'method',
      start: 10,
      end: 20,
      signature: 'putBlob(v)',
    });
  });

  it('Trace content 文本优先；无文本时退回 details JSON', () => {
    expect(parseTraceEnvelope(traceEnvelope('ipc:demo → handler'), 'ipc:demo').text).toContain('handler');
    const fallback = parseTraceEnvelope({ ok: true, result: { content: [], details: { kind: 'page', found: false } } }, 'page:a.html');
    expect(fallback.text).toContain('"kind": "page"');
  });
});

describe('PTY 输出与文本工具', () => {
  it('parsePtyFindOutput 解析 %y\\t%f\\t%s 并忽略错误行', () => {
    const entries = parsePtyFindOutput('d\tsrc\t0\nf\tREADME.md\t128\nfind: warning: x', '');
    expect(entries.map((entry) => [entry.name, entry.type, entry.size])).toEqual([
      ['src', 'dir', 0],
      ['README.md', 'file', 128],
    ]);
    expect(parsePtyFindOutput('f\ta.ts\t10', 'sub').map((entry) => entry.path)).toEqual(['sub/a.ts']);
  });

  it('extractMarkedOutput 取 BEGIN/END 之间的输出（忽略命令回显行）', () => {
    const text = [
      '$ echo __PF_SOURCE_BEGIN__; cd /x; find .; echo __PF_SOURCE_END__',
      '__PF_SOURCE_BEGIN__',
      'f\ta\t1',
      '__PF_SOURCE_END__',
    ].join('\r\n');
    expect(extractMarkedOutput(text)).toBe('f\ta\t1');
    expect(extractMarkedOutput('no markers')).toBe('no markers');
  });

  it('toShellPath 支持 Windows 盘符与 POSIX；shellQuote 单引号安全', () => {
    expect(toShellPath('D:\\VCPChat\\demo')).toBe('/d/VCPChat/demo');
    expect(toShellPath('/home/user/proj')).toBe('/home/user/proj');
    expect(shellQuote('a b')).toBe("'a b'");
    expect(shellQuote("it's")).toBe(`'it'\\''s'`);
  });

  it('路径校验与拼接', () => {
    expect(normalizeRelativePath('./src//a.ts')).toBe('src/a.ts');
    expect(normalizeRelativePath('/abs/path')).toBe('abs/path');
    expect(ensureRelativePath(' src/a.ts ')).toBe('src/a.ts');
    expect(() => ensureRelativePath('../etc/passwd')).toThrow('不能包含 ..');
    expect(() => ensureRelativePath('/etc/passwd')).toThrow('不能以 / 开头');
    expect(() => ensureRelativePath('D:\\x')).toThrow('Windows 绝对路径');
    expect(joinProjectPath('/root/demo', 'a/b')).toBe('/root/demo/a/b');
    expect(joinProjectPath('D:\\root\\demo', 'a/b')).toBe('D:\\root\\demo\\a\\b');
  });

  it('行数 / 文本等价 / 展示工具', () => {
    expect(countContentLines('a\nb\n')).toBe(2);
    expect(countContentLines('a\nb')).toBe(2);
    expect(countContentLines('')).toBe(0);
    expect(sameFileText('a\r\nb\r\n', 'a\nb')).toBe(true);
    expect(sameFileText('a', 'b')).toBe(false);
    expect(splitSearchTokens('  SRC   main ')).toEqual(['src', 'main']);
    expect(languageLabel('src/App.vue')).toBe('Vue');
    expect(formatSourceSize(2048)).toBe('2.0 KB');
    expect(formatSourceSize(undefined)).toBe('—');
  });

  it('flattenTreeRows 依据展开集合展开目录并标记 loading', () => {
    const rows = flattenTreeRows(
      {
        '': {
          loaded: true,
          loading: false,
          error: '',
          entries: [
            { name: 'src', path: 'src', type: 'dir' },
            { name: 'a.ts', path: 'a.ts', type: 'file' },
          ],
        },
        src: { loaded: true, loading: true, error: '', entries: [{ name: 'b.ts', path: 'src/b.ts', type: 'file' }] },
      },
      ['src'],
    );
    expect(rows.map((row) => [row.path, row.depth, row.open, row.loading])).toEqual([
      ['src', 0, true, true],
      ['src/b.ts', 1, false, false],
      ['a.ts', 0, false, false],
    ]);
  });

  it('extractReviewWarnings 从 EditCode review/issues 里抽取提示', () => {
    expect(extractReviewWarnings({ status: 'ok', review: { issues: [{ message: '缺少测试' }, '行尾空格'] } })).toEqual([
      '缺少测试',
      '行尾空格',
    ]);
    expect(extractReviewWarnings({ status: 'ok' })).toEqual([]);
  });
});
