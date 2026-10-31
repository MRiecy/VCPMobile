import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { verifyAndPackage } from './verify-archive.mjs';

const digest = data => createHash('sha256').update(data).digest('hex');
const mac = process.platform === 'darwin';

async function writePlist(file, info) {
  const input = JSON.stringify(info);
  if (mac) execFileSync('plutil', ['-convert', 'binary1', '-o', file, '--', '-'], { input });
  else await writeFile(file, input);
}

async function fixture(t, bundleVersion) {
  const work = await mkdtemp(join(tmpdir(), 'vcp-ios-archive-'));
  t.after(() => rm(work, { force: true, recursive: true }));
  const root = join(work, 'src-tauri');
  const project = join(root, 'gen', 'Apple Project');
  const archive = join(project, 'build', 'sample_iOS.xcarchive');
  const app = join(archive, 'Products', 'Applications', 'Example.app');
  const share = join(app, 'PlugIns', 'VCPShare.appex');
  const output = join(work, 'publish');
  const sources = join(root, 'runtime-assets', 'voice');
  const assets = join(sources, 'assets', 'voice');
  for (const dir of [assets, share, join(app, 'voice')]) await mkdir(dir, { recursive: true });

  const main = {
    CFBundleIdentifier: 'com.example.test', CFBundleExecutable: 'Example',
    CFBundleShortVersionString: '1.2.3', CFBundleVersion: bundleVersion ?? '1.2.3',
    VCPShareAppGroup: 'group.com.example.test', MinimumOSVersion: '26.0',
    DTSDKName: 'iphoneos26.2', DTXcodeBuild: '17C52',
  };
  const extension = {
    ...main, CFBundleIdentifier: 'com.example.test.share', CFBundleExecutable: 'VCPShare',
    NSExtension: { NSExtensionPointIdentifier: 'com.apple.share-services' },
  };
  await writeFile(join(root, 'tauri.conf.json'), JSON.stringify({ identifier: main.CFBundleIdentifier, version: '1.2.3' }));
  await writeFile(join(root, 'tauri.ios.conf.json'), JSON.stringify({ bundle: { iOS: { minimumSystemVersion: '26.0', bundleVersion } } }));
  await writePlist(join(root, 'Info.ios.plist'), { VCPShareAppGroup: main.VCPShareAppGroup });
  for (const [dir, info] of [[app, main], [share, extension]]) {
    await writePlist(join(dir, 'Info.plist'), info);
    await writeFile(join(dir, info.CFBundleExecutable), 'fixture executable');
    await writePlist(join(dir, 'PrivacyInfo.xcprivacy'), {});
  }
  const resources = {
    'silero_vad.ort': 'fixture model', 'SILERO-LICENSE': 'silero license',
    'ONNXRUNTIME-LICENSE': 'runtime license', 'ONNXRUNTIME-ThirdPartyNotices.txt': 'third-party notices',
  };
  for (const [name, data] of Object.entries(resources)) {
    await writeFile(join(assets, name), data);
    await writeFile(join(app, 'voice', name), data);
  }
  await writeFile(join(sources, 'runtime.lock.json'), JSON.stringify({ ortModelSha256: digest(resources['silero_vad.ort']) }));

  // Metadata validation, file hashing and publication use the real temporary tree.
  // macOS also exercises binary plists and ditto; Windows substitutes only those Apple commands.
  let compressions = 0;
  const run = async (command, args) => {
    if (command === 'xcodebuild') return 'Xcode 26.2\nBuild version 17C52';
    if (command === 'xcrun') return '26.2';
    if (command === 'ditto') compressions++;
    if (mac) return execFileSync(command, args, { encoding: 'utf8' });
    if (command === 'plutil') return readFile(args.at(-1), 'utf8');
    if (command === 'ditto') return writeFile(args.at(-1), 'fixture archive');
    throw new Error(`Unexpected command: ${command}`);
  };
  const publish = (runner = run) => verifyAndPackage(project, output, { root, commit: 'test-revision', profile: 'release' }, runner);
  return { work, project, sources, assets, app, share, main, extension, output, publish, run, compressions: () => compressions };
}

for (const bundleVersion of [undefined, '42']) {
  test(`publishes one checked archive and its SHA-256 manifest (build ${bundleVersion ?? 'from version'})`, async t => {
    const f = await fixture(t, bundleVersion);
    // A nested archive from a different build tool must not become the Tauri product.
    await mkdir(join(f.project, 'DerivedData', 'wrong.xcarchive'), { recursive: true });
    await f.publish();
    const manifest = JSON.parse(await readFile(join(f.output, 'manifest.json'), 'utf8'));
    assert.deepEqual((await readdir(f.output)).sort(), ['manifest.json', 'sample_iOS.xcarchive.zip']);
    const zip = await readFile(join(f.output, manifest.archive.file));
    assert.equal(manifest.archive.sha256, digest(zip));
    assert.equal(manifest.archive.bytes, zip.length);
    assert.equal(manifest.commit, 'test-revision');
    assert.equal(manifest.profile, 'release');
    assert.equal(manifest.app.bundleVersion, bundleVersion ?? '1.2.3');
    assert.equal(manifest.share.identifier, 'com.example.test.share');
    assert.equal(manifest.share.minimumOSVersion, '26.0');
    assert.equal(manifest.app.xcodeBuild, '17C52');
    assert.equal(manifest.share.sdkName, 'iphoneos26.2');
    assert.equal(manifest.voiceResources.find(r => r.file === 'voice/silero_vad.ort').sha256, digest('fixture model'));
    assert.equal(f.compressions(), 1);
  });
}

const wrongPlists = [
  ['app', 'CFBundleIdentifier', 'com.example.foreign'],
  ['share', 'CFBundleIdentifier', 'com.example.other-extension'],
  ['app', 'CFBundleShortVersionString', '1.0.0'],
  ['share', 'CFBundleVersion', '41'],
  ['share', 'VCPShareAppGroup', 'group.other'],
  ['app', 'DTSDKName', 'iphonesimulator26.2'],
  ['share', 'DTXcodeBuild', '16F6'],
  ['app', 'MinimumOSVersion', '15.0'],
  ['share', 'NSExtension', { NSExtensionPointIdentifier: 'com.apple.widgetkit-extension' }],
];
for (const [target, key, value] of wrongPlists) {
  test(`rejects wrong ${target} ${key} before compression`, async t => {
    const f = await fixture(t, '42');
    const info = target === 'app' ? f.main : f.extension;
    await writePlist(join(f[target], 'Info.plist'), { ...info, [key]: value });
    await assert.rejects(f.publish(), key === 'NSExtension' ? /Share Extension point/ : new RegExp(key));
    assert.equal(f.compressions(), 0);
    assert.equal(existsSync(f.output), false);
  });
}

test('rejects a different model even when its staged source was also replaced', async t => {
  const f = await fixture(t);
  for (const dir of [f.assets, join(f.app, 'voice')]) await writeFile(join(dir, 'silero_vad.ort'), 'different model');
  await assert.rejects(f.publish(), /silero_vad\.ort/);
  assert.equal(f.compressions(), 0);
  assert.equal(existsSync(f.output), false);
});

for (const [target, file] of [['app', 'voice/ONNXRUNTIME-LICENSE'], ['share', 'PrivacyInfo.xcprivacy'], ['app', 'Example']]) {
  test(`missing ${target} ${file} prevents publication`, async t => {
    const f = await fixture(t);
    await rm(join(f[target], file));
    await assert.rejects(f.publish(), /ENOENT/);
    assert.equal(f.compressions(), 0);
    assert.equal(existsSync(f.output), false);
  });
}

test('rejects ambiguous Tauri archive selection', async t => {
  const f = await fixture(t);
  await mkdir(join(f.project, 'build', 'stale.xcarchive'));
  await assert.rejects(f.publish(), /Expected one .xcarchive/);
  assert.equal(existsSync(f.output), false);
});

test('the CLI exits unsuccessfully when a build has no archive to publish', async t => {
  const f = await fixture(t);
  await rm(join(f.project, 'build', 'sample_iOS.xcarchive'), { recursive: true });
  const cli = spawnSync(process.execPath, [fileURLToPath(new URL('./verify-archive.mjs', import.meta.url)), f.project, f.output], {
    env: { ...process.env, GITHUB_SHA: 'test-revision', IOS_ARCHIVE_PROFILE: 'debug' }, encoding: 'utf8',
  });
  assert.equal(cli.status, 1);
  assert.match(cli.stderr, /Expected one .xcarchive/);
  assert.equal(existsSync(f.output), false);
});

test('a compressor failure after writing bytes never publishes a partial archive', async t => {
  const f = await fixture(t);
  await assert.rejects(f.publish(async (command, args) => {
    if (command !== 'ditto') return f.run(command, args);
    await writeFile(args.at(-1), 'partial zip');
    throw new Error('ditto failed');
  }), /ditto failed/);
  assert.deepEqual(await readdir(f.work), ['src-tauri']);
});
