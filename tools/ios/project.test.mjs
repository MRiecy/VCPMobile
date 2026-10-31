import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, readdir } from 'node:fs/promises';
import { basename, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { Window } from 'happy-dom';

const tauri = fileURLToPath(new URL('../../src-tauri/', import.meta.url));
const apple = join(tauri, 'gen/apple');
const extension = join(apple, 'VCPShare');

test('every iPhone, iPad and marketing icon slot references a PNG of the declared pixel size', async () => {
  const icons = join(apple, 'Assets.xcassets/AppIcon.appiconset');
  const { images } = JSON.parse(await readFile(join(icons, 'Contents.json'), 'utf8'));
  const slots = new Set();
  for (const { idiom, size, scale, filename } of images) {
    const slot = [idiom, size, scale].join('/');
    assert(!slots.has(slot), slot);
    slots.add(slot);
    assert.equal(basename(filename), filename);
    const png = await readFile(join(icons, filename));
    assert.equal(png.subarray(0, 8).toString('hex'), '89504e470d0a1a0a', filename);
    assert.equal(png.toString('ascii', 12, 16), 'IHDR', filename);
    const dimensions = size.split('x').map(value => Number(value) * Number(scale.slice(0, -1)));
    assert.deepEqual([png.readUInt32BE(16), png.readUInt32BE(20)], dimensions, slot);
  }
  assert.deepEqual(new Set(images.map(image => image.idiom)), new Set(['iphone', 'ipad', 'ios-marketing']));
});

test('the app and the extension open the group granted by their shared entitlement', async () => {
  const window = new Window();
  try {
    const parser = new window.DOMParser();
    async function plistValue(path, key) {
      const xml = parser.parseFromString(await readFile(path, 'utf8'), 'application/xml');
      assert.equal(xml.documentElement.localName, 'plist');
      const entry = [...xml.documentElement.firstElementChild.children].find(item =>
        item.localName === 'key' && item.textContent === key);
      assert(entry, `${path}: ${key}`);
      return entry.nextElementSibling;
    }
    const group = await plistValue(join(tauri, 'Info.ios.plist'), 'VCPShareAppGroup');
    const shareGroup = await plistValue(join(extension, 'Info.plist'), 'VCPShareAppGroup');
    const granted = await plistValue(join(extension, 'AppGroups.entitlements'), 'com.apple.security.application-groups');
    assert.equal(group.localName, 'string');
    assert.equal(shareGroup.localName, 'string');
    assert.equal(shareGroup.textContent, group.textContent);
    assert.equal(granted.localName, 'array');
    assert.deepEqual([...granted.children].map(value => value.textContent), [group.textContent]);
  } finally {
    await window.happyDOM.close();
  }
});

test('the generated targets assign Swift sources, embed the share extension and own separate bundle resources', {
  skip: (process.platform !== 'darwin' || !process.env.IOS_ARCHIVE_PROFILE) &&
    'The generated device project is inspected in the archive workflow after ios init',
}, async () => {
  const projects = (await readdir(apple)).filter(name => name.endsWith('.xcodeproj'));
  assert.equal(projects.length, 1);
  const converted = execFileSync('plutil', ['-convert', 'json', '-o', '-', join(apple, projects[0], 'project.pbxproj')], { encoding: 'utf8' });
  const { objects, rootObject } = JSON.parse(converted);
  const targets = objects[rootObject].targets.map(id => objects[id]);
  const app = targets.find(target => target.productType === 'com.apple.product-type.application');
  const share = targets.find(target => target.productType === 'com.apple.product-type.app-extension');
  assert(app && share);
  function phaseFiles(target, phase, destination) {
    return target.buildPhases.map(id => objects[id])
      .filter(item => item.isa === phase && (!destination || String(item.dstSubfolderSpec) === destination))
      .flatMap(item => item.files.map(id => objects[id].fileRef));
  }
  const appSources = phaseFiles(app, 'PBXSourcesBuildPhase');
  const shareSources = phaseFiles(share, 'PBXSourcesBuildPhase');
  assert(shareSources.length > 0);
  assert.deepEqual(shareSources.map(id => basename(objects[id].path)).sort(),
    (await readdir(extension)).filter(name => name.endsWith('.swift')).sort());
  assert(shareSources.every(id => !appSources.includes(id)));
  // Xcode's PlugIns destination is PBXCopyFilesBuildPhase.dstSubfolderSpec 13.
  assert(phaseFiles(app, 'PBXCopyFilesBuildPhase', '13').includes(share.productReference));
  const resourceFiles = target => phaseFiles(target, 'PBXResourcesBuildPhase')
    .map(id => objects[id]).filter(item => item.isa === 'PBXFileReference');
  const appResources = resourceFiles(app);
  const shareResources = resourceFiles(share);
  const appPrivacy = appResources.filter(ref => basename(ref.path) === 'PrivacyInfo.xcprivacy');
  const sharePrivacy = shareResources.filter(ref => basename(ref.path) === 'PrivacyInfo.xcprivacy');
  assert.equal(appPrivacy.length, 1);
  assert.equal(sharePrivacy.length, 1);
  assert.notEqual(appPrivacy[0], sharePrivacy[0]);
  assert(appResources.some(ref => basename(ref.path) === 'voice' && (ref.lastKnownFileType ?? ref.explicitFileType) === 'folder'));
  for (const target of [app, share]) {
    for (const config of objects[target.buildConfigurationList].buildConfigurations) {
      assert.equal(objects[config].buildSettings.CODE_SIGN_ENTITLEMENTS, 'VCPShare/AppGroups.entitlements');
    }
  }
});
