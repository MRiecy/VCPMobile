import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const profile = process.argv[2];
if (!['release', 'debug'].includes(profile)) throw new Error('Expected archive profile: release or debug');
if (process.platform !== 'darwin' || process.env.GITHUB_ACTIONS !== 'true') {
  throw new Error('Run the iOS Device Archive workflow on the macOS runner.');
}

const root = new URL('../../', import.meta.url);
const config = JSON.parse(readFileSync(new URL('src-tauri/tauri.ios.conf.json', root), 'utf8'));
const xcrun = (...args) => execFileSync('xcrun', args, { encoding: 'utf8' }).trim();
const env = {
  ...process.env,
  IPHONEOS_DEPLOYMENT_TARGET: config.bundle.iOS.minimumSystemVersion,
  SDKROOT: xcrun('--sdk', 'iphoneos', '--show-sdk-path'),
  CC_aarch64_apple_ios: xcrun('--sdk', 'iphoneos', '--find', 'clang'),
  CXX_aarch64_apple_ios: xcrun('--sdk', 'iphoneos', '--find', 'clang++'),
  AR_aarch64_apple_ios: xcrun('--find', 'ar'),
};
for (const key of [
  'LIBSQLITE3_SYS_USE_PKG_CONFIG', 'ZSTD_SYS_USE_PKG_CONFIG', 'RING_PREGENERATE_ASM',
  'APPLE_DEVELOPMENT_TEAM', 'IOS_CERTIFICATE', 'IOS_CERTIFICATE_PASSWORD', 'IOS_MOBILE_PROVISION',
  'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_PROVISIONING_PROFILE',
  'APPLE_API_KEY', 'APPLE_API_KEY_PATH', 'APPLE_API_ISSUER',
]) delete env[key];
if (profile === 'debug') {
  env.CARGO_PROFILE_DEV_DEBUG = '1';
  env.CARGO_PROFILE_DEV_STRIP = 'false';
}
execFileSync('pnpm', [
  'tauri', 'ios', 'build', ...(profile === 'debug' ? ['--debug'] : []),
  '--target', 'aarch64', '--archive-only', '--no-sign', '--ci', '--', '--locked',
], { cwd: fileURLToPath(root), env, stdio: 'inherit' });
