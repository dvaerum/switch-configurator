// E2E browser tests for switch-configurator-ui using Playwright
// Run: npx playwright test tests/e2e/test-ui.mjs
// Requires: backend + UI running (see test-e2e.sh)

import { test, expect } from '@playwright/test';

const UI_URL = process.env.UI_URL || 'http://127.0.0.1:8099';

test.describe('Dashboard', () => {
  test('shows switch cards with correct data', async ({ page }) => {
    await page.goto(UI_URL);

    // Should show "Switches" heading
    await expect(page.locator('h2')).toContainText('Switches');

    // Should show at least one switch card
    const cards = page.locator('.card');
    await expect(cards).not.toHaveCount(0);

    // First card should have the demo switch hostname
    await expect(cards.first()).toContainText('demo-switch-1');

    // Should show model and IP
    await expect(cards.first()).toContainText('Aruba2930F');
    await expect(cards.first()).toContainText('192.168.1.1');

    // Should show VLAN/port count
    await expect(cards.first()).toContainText('VLANs');
    await expect(cards.first()).toContainText('ports');
  });

  test('switch card links to detail view', async ({ page }) => {
    await page.goto(UI_URL);
    await page.locator('.card').first().click();
    await expect(page).toHaveURL(/\/switch\/demo-sw-01/);
    await expect(page.locator('h2')).toContainText('demo-switch-1');
  });
});

test.describe('Switch Detail View', () => {
  test('shows switch header info', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('h2')).toContainText('demo-switch-1');
    await expect(page.locator('body')).toContainText('Aruba2930F');
    await expect(page.locator('body')).toContainText('192.168.1.1');
  });

  test('has all tabs', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('.tab')).toHaveCount(6);
    await expect(page.locator('.tabs')).toContainText('Overview');
    await expect(page.locator('.tabs')).toContainText('VLANs');
    await expect(page.locator('.tabs')).toContainText('Ports');
    await expect(page.locator('.tabs')).toContainText('Mirrors');
    await expect(page.locator('.tabs')).toContainText('SNMP');
    await expect(page.locator('.tabs')).toContainText('Config Sources');
  });

  test('has Edit button', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('button:has-text("Edit")')).toBeVisible();
  });
});

test.describe('VLANs Tab', () => {
  test('shows VLAN table with correct data', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/vlans`);

    // Should show VLAN table
    const table = page.locator('table');
    await expect(table).toBeVisible();

    // Should have VLAN rows
    await expect(page.locator('td')).toContainText(['management']);
    await expect(page.locator('td')).toContainText(['users']);
    await expect(page.locator('td')).toContainText(['servers']);
  });
});

test.describe('Ports Tab', () => {
  test('shows port table with correct data', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/ports`);

    const table = page.locator('table');
    await expect(table).toBeVisible();

    // Should show port data
    await expect(page.locator('body')).toContainText('Uplink');
    await expect(page.locator('body')).toContainText('User ports');
    await expect(page.locator('body')).toContainText('Trunk');
  });
});

test.describe('SNMP Tab', () => {
  test('shows SNMP communities and traps', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/snmp`);

    // Should show community
    await expect(page.locator('body')).toContainText('public');
    await expect(page.locator('body')).toContainText('operator');

    // Should show trap receiver
    await expect(page.locator('body')).toContainText('192.168.1.100');

    // Should show enabled traps
    await expect(page.locator('body')).toContainText('mac-notify');
    await expect(page.locator('body')).toContainText('link-change');
  });
});

test.describe('Edit Workflow', () => {
  test('Edit button starts draft and shows edit view', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);

    // Click Edit button
    await page.locator('button:has-text("Edit")').click();

    // Should navigate to edit VLANs view
    await expect(page).toHaveURL(/\/edit\/vlans/);
    await expect(page.locator('body')).toContainText('Draft Mode');

    // Should show editable VLAN form
    await expect(page.locator('input[name="name"]').first()).toBeVisible();
  });

  test('Discard button returns to read-only view', async ({ page }) => {
    // Start a draft
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await expect(page).toHaveURL(/\/edit\/vlans/);

    // Click Discard
    await page.locator('a:has-text("Discard")').click();

    // Should return to detail view (no Draft Mode badge)
    await expect(page).toHaveURL(/\/switch\/demo-sw-01/);
  });

  test('Save dialog shows filename and priority fields', async ({ page }) => {
    // Start a draft
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();

    // Click Save
    await page.locator('a:has-text("Save")').first().click();

    // Should show save dialog
    await expect(page.locator('body')).toContainText('Save Configuration Overlay');
    await expect(page.locator('input[name="filename"]')).toBeVisible();
    await expect(page.locator('input[name="priority"]')).toBeVisible();
  });
});
