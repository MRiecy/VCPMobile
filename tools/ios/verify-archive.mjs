import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const tauriRoot = fileURLToPath(new URL('../../src-tauri/', import.meta.url));
const readJson = async (file) => JSON.parse(await readFile(file, 'utf8'));
const appleTool = async (command, args) => (await execute(command, args, { encoding: 'utf8' })).stdout.trim();
const readPlist = async (file, run) => JSON.parse(await run('plutil', ['-convert', 'json', '-o', '-', '--', file]));

async function sha256(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

async function artifactRecord(path) {
  return { file: basename(path), bytes: (await stat(path)).size, sha256: await sha256(path) };
}

async function verifyIpa(ipa, appName, mainExecutable, shareExecutable, run) {
  const entries = (await run('unzip', ['-Z1', ipa])).trimEnd().split(/\r?\n/);
  const app = `Payload/${appName}/`;
  if (entries.some(entry => entry !== 'Payload/' && !entry.startsWith(app))) {
    throw new Error(`IPA must contain exactly Payload/${appName}`);
  }
  for (const path of ['Info.plist', mainExecutable, 'PlugIns/VCPShare.appex/Info.plist', `PlugIns/VCPShare.appex/${shareExecutable}`]) {
    if (!entries.includes(app + path)) throw new Error(`Missing IPA entry: ${app}${path}`);
  }
}

async function onlyBundle(parent, suffix) {
  const entries = (await readdir(parent, { withFileTypes: true }))
    .filter(entry => entry.isDirectory() && entry.name.endsWith(suffix));
  if (entries.length !== 1) throw new Error(`Expected one ${suffix} under ${parent}, found ${entries.length}`);
  return join(parent, entries[0].name);
}

async function verifyBundle(bundle, expected, run) {
  const info = await readPlist(join(bundle, 'Info.plist'), run);
  for (const [key, value] of Object.entries(expected)) {
    if (!value || info[key] !== value) {
      throw new Error(`${basename(bundle)}: expected ${key}=${value}, found ${info[key]}`);
    }
  }
  const executable = info.CFBundleExecutable;
  if (typeof executable !== 'string' || !executable || basename(executable) !== executable
      || !(await stat(join(bundle, executable))).isFile()) {
    throw new Error(`${basename(bundle)}: missing bundle executable`);
  }
  if (!(await stat(join(bundle, 'PrivacyInfo.xcprivacy'))).isFile()) {
    throw new Error(`${basename(bundle)}: missing privacy manifest`);
  }
  return info;
}

async function inspectMachO(binary, run) {
  let architectures;
  try {
    const output = (await run('xcrun', ['lipo', '-archs', binary])).trim();
    if (!output) throw new Error('No architectures reported');
    architectures = output.split(/\s+/);
  } catch (error) {
    return { error: { tool: 'lipo', reason: error.message } };
  }

  const slices = await Promise.all(architectures.map(async architecture => {
    try {
      const output = await run('xcrun', ['vtool', '-arch', architecture, '-show-build', binary]);
      const platforms = Array.from(output.matchAll(/^[ \t]*platform[ \t]+(\S.*)$/gm), match => match[1].trim());
      if (!platforms.length) throw new Error('No build platform reported');
      return { architecture, platforms };
    } catch (error) {
      return { architecture, error: { tool: 'vtool', reason: error.message } };
    }
  }));
  return { slices };
}

async function verifyVoice(app, root) {
  const voiceRoot = join(root, 'runtime-assets', 'voice');
  const sources = join(voiceRoot, 'assets', 'voice');
  const names = (await readdir(sources, { withFileTypes: true }))
    .filter(entry => entry.isFile()).map(entry => entry.name).sort();
  for (const name of ['silero_vad.ort', 'SILERO-LICENSE', 'ONNXRUNTIME-LICENSE', 'ONNXRUNTIME-ThirdPartyNotices.txt']) {
    if (!names.includes(name)) throw new Error(`Missing voice build input: ${name}`);
  }
  const lock = await readJson(join(voiceRoot, 'runtime.lock.json'));
  const records = [];
  for (const name of names) {
    const file = join(app, 'voice', name);
    const digest = await sha256(file);
    if (digest !== await sha256(join(sources, name)) || (name === 'silero_vad.ort' && digest !== lock.ortModelSha256)) {
      throw new Error(`voice/${name} does not match its build input`);
    }
    records.push({ file: `voice/${name}`, bytes: (await stat(file)).size, sha256: digest });
  }
  return records;
}

export async function verifyAndPackage(project, destination, { commit, profile, root = tauriRoot }, run = appleTool) {
  if (!commit || !['debug', 'release'].includes(profile)) throw new Error('Expected GITHUB_SHA and IOS_ARCHIVE_PROFILE=debug|release');
  const output = resolve(destination);
  if (existsSync(output)) throw new Error(`Output directory already exists: ${output}`);

  // Tauri --archive-only writes build/<scheme>.xcarchive for either profile.
  const archive = await onlyBundle(join(resolve(project), 'build'), '.xcarchive');
  const app = await onlyBundle(join(archive, 'Products', 'Applications'), '.app');
  const extension = join(app, 'PlugIns', 'VCPShare.appex');
  const config = await readJson(join(root, 'tauri.conf.json'));
  const ios = (await readJson(join(root, 'tauri.ios.conf.json'))).bundle.iOS;
  const group = (await readPlist(join(root, 'Info.ios.plist'), run)).VCPShareAppGroup;
  const sdkVersion = await run('xcrun', ['--sdk', 'iphoneos', '--show-sdk-version']);
  const xcode = await run('xcodebuild', ['-version']);
  const xcodeBuild = /^Build version (\S+)$/m.exec(xcode)?.[1];
  const common = {
    CFBundleShortVersionString: config.version,
    CFBundleVersion: ios.bundleVersion ?? config.version,
    MinimumOSVersion: ios.minimumSystemVersion,
    DTSDKName: `iphoneos${sdkVersion}`,
    DTXcodeBuild: xcodeBuild,
    VCPShareAppGroup: group,
  };
  const mainInfo = await verifyBundle(app, { ...common, CFBundleIdentifier: config.identifier }, run);
  const shareInfo = await verifyBundle(extension, { ...common, CFBundleIdentifier: `${config.identifier}.share` }, run);
  if (shareInfo.NSExtension?.NSExtensionPointIdentifier !== 'com.apple.share-services') {
    throw new Error('VCPShare.appex: invalid Share Extension point');
  }
  const resources = await verifyVoice(app, root);
  const bundleRecord = async (bundle, info) => ({
    name: basename(bundle), identifier: info.CFBundleIdentifier, executable: info.CFBundleExecutable,
    version: info.CFBundleShortVersionString, bundleVersion: info.CFBundleVersion,
    minimumOSVersion: info.MinimumOSVersion, appGroup: info.VCPShareAppGroup,
    sdkName: info.DTSDKName, xcodeBuild: info.DTXcodeBuild,
    machO: await inspectMachO(join(bundle, info.CFBundleExecutable), run),
  });
  const [appRecord, shareRecord] = await Promise.all([bundleRecord(app, mainInfo), bundleRecord(extension, shareInfo)]);

  // Publish the archive, both install inputs and their checksums as one directory.
  await mkdir(dirname(output), { recursive: true });
  const stage = await mkdtemp(join(dirname(output), '.ios-archive-'));
  try {
    const archiveZip = join(stage, `${basename(archive)}.zip`);
    await run('ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', archive, archiveZip]);
    const appName = basename(app);
    const prefix = `VCPMobile_v${appRecord.version}_ios-arm64${profile === 'debug' ? '-debug' : ''}`;
    if (prefix.includes('/') || prefix.includes('\\')) throw new Error('Invalid version for package filename');
    const appZip = join(stage, `${prefix}.app.zip`);
    const ipa = join(stage, `${prefix}.unsigned.ipa`);
    const scratch = await mkdtemp(join(stage, '.packaging-'));
    await run('ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', app, appZip]);
    const payload = join(scratch, 'Payload');
    await mkdir(payload);
    await run('ditto', [app, join(payload, appName)]);
    // iOS Payload contains the data fork and Unix permissions; omit macOS resource sidecars.
    await run('ditto', ['-c', '-k', '--norsrc', '--keepParent', payload, ipa]);
    await verifyIpa(ipa, appName, mainInfo.CFBundleExecutable, shareInfo.CFBundleExecutable, run);
    await rm(scratch, { recursive: true });

    const [archiveArtifact, appArtifact, ipaArtifact] = await Promise.all([archiveZip, appZip, ipa].map(artifactRecord));
    for (const artifact of [appArtifact, ipaArtifact]) {
      await writeFile(join(stage, `${artifact.file}.sha256`), `${artifact.sha256}  ${artifact.file}\n`);
    }
    const manifest = {
      commit, profile, xcode, sdkVersion,
      archive: archiveArtifact, appZip: appArtifact, ipa: ipaArtifact,
      app: appRecord,
      share: { ...shareRecord, extensionPoint: shareInfo.NSExtension.NSExtensionPointIdentifier },
      voiceResources: resources,
    };
    await writeFile(join(stage, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
    await rename(stage, output);
    return manifest;
  } catch (error) {
    await rm(stage, { recursive: true, force: true });
    throw error;
  }
}

if (import.meta.main) {
  const [project, destination] = process.argv.slice(2);
  if (process.argv.length !== 4) throw new Error('Usage: node tools/ios/verify-archive.mjs <apple-project> <output-directory>');
  const result = await verifyAndPackage(project, destination, {
    commit: process.env.GITHUB_SHA, profile: process.env.IOS_ARCHIVE_PROFILE,
  });
  for (const artifact of [result.archive, result.appZip, result.ipa]) {
    console.log(`Verified ${artifact.file} (${artifact.sha256})`);
  }
}
