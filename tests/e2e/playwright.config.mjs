import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  timeout: 30000,
  retries: 0,
  use: {
    headless: true,
    browserName: 'firefox',
    screenshot: 'only-on-failure',
  },
  reporter: [['list']],
});
