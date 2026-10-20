import js from '@eslint/js';
import { defineConfig, globalIgnores } from 'eslint/config';
import tsPlugin from '@typescript-eslint/eslint-plugin';
import tsParser from '@typescript-eslint/parser';
import prettier from 'eslint-config-prettier';
import vue from 'eslint-plugin-vue';
import vueParser from 'vue-eslint-parser';
import globals from 'globals';

export default defineConfig(
  globalIgnores([
    '**/node_modules/**',
    '**/target/**',
    'dist/**',
    'dist-ssr/**',
    'coverage/**',
    'src-tauri/gen/**',
    'src-tauri/plugins/**/android/build/**',
    'src-tauri/plugins/**/android/.tauri/**',
    'src-tauri/plugins/**/ios/.build/**',
    'src-tauri/plugins/**/ios/.swiftpm/**',
    'src-tauri/plugins/**/.tauri/**',
    // Repository .gitignore identifies these as local captures, dependency
    // caches, archived material and agent/editor state rather than source.
    'tests/perf/reports/**',
    '.cargo/**',
    'scripts/**',
    'vector/**',
    'plans/**',
    '.agent/**',
    '.claude/**',
    '.kilo/**',
    '.kilocode/**',
    '.kimi/**',
    '.antigravitycli/**',
    '.vscode/**',
    '.idea/**',
    'logs/**',
    'temp/**',
    'releases/**',
  ]),
  {
    name: 'project/correctness',
    files: ['**/*.{js,mjs,cjs,ts,vue}'],
    extends: [js.configs.recommended],
    linterOptions: {
      reportUnusedDisableDirectives: 'error',
    },
  },
  {
    name: 'project/typescript',
    files: ['**/*.{ts,vue}'],
    extends: [tsPlugin.configs['flat/recommended']],
    languageOptions: {
      parser: tsParser,
    },
    rules: {
      // IPC payloads, graphics adapters and test doubles still have explicit
      // dynamic boundaries. Retyping them is separate from dependency upgrades;
      // tsc strict continues to reject implicit any.
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-unused-vars': ['error', {
        argsIgnorePattern: '^_',
        caughtErrorsIgnorePattern: '^_',
        varsIgnorePattern: '^_',
      }],
      // TypeScript resolves names and type-only declarations, including SFCs.
      'no-undef': 'off',
    },
  },
  {
    name: 'project/vue',
    files: ['**/*.vue'],
    extends: [vue.configs['flat/recommended']],
    languageOptions: {
      parser: vueParser,
      parserOptions: {
        parser: tsParser,
      },
    },
    rules: {
      // Keep existing template spelling and layout; formatting is not a lint
      // migration. The remaining recommended Vue correctness rules stay active.
      'vue/attributes-order': 'off',
      'vue/first-attribute-linebreak': 'off',
      'vue/attribute-hyphenation': 'off',
      'vue/v-on-event-hyphenation': 'off',
      // An optional prop can intentionally retain Vue's undefined default.
      'vue/require-default-prop': 'off',
    },
  },
  {
    name: 'project/browser',
    files: ['src/**/*.{js,ts,vue}', 'src-tauri/plugins/vcp-mobile/guest-js/**/*.ts'],
    languageOptions: {
      globals: globals.browser,
    },
  },
  {
    name: 'project/node',
    files: [
      '*.{js,mjs,cjs,ts}',
      'tools/**/*.{js,mjs,cjs,ts}',
      'tests/**/*.{js,mjs,cjs,ts}',
      '.github/**/*.{js,mjs,cjs,ts}',
      'plan/**/*.{js,mjs,cjs,ts}',
    ],
    languageOptions: {
      globals: globals.node,
    },
  },
  // Prettier owns whitespace and punctuation; do not run it as an ESLint rule.
  prettier,
);
