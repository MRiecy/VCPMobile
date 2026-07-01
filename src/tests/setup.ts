import { afterEach } from 'vitest';
import { config } from '@vue/test-utils';
import './mocks/browser';
import './mocks/tauri';

config.global.stubs = {
  Transition: false,
  Teleport: false,
};

afterEach(() => {
  document.body.innerHTML = '';
  document.head.innerHTML = '';
});
