// E2E browser tests for switch-configurator-ui using Playwright (Firefox headless)
// Run: UI_URL=http://127.0.0.1:8099 npx playwright test --config=playwright.config.mjs

import { test, expect } from '@playwright/test';

const UI_URL = process.env.UI_URL || 'http://127.0.0.1:8099';

// ============================================================================
// Dashboard
// ============================================================================

test.describe('Dashboard', () => {
  test('shows switch cards with correct data', async ({ page }) => {
    await page.goto(UI_URL);
    await expect(page.locator('h2')).toContainText('Switches');

    const cards = page.locator('.card');
    await expect(cards).not.toHaveCount(0);
    await expect(cards.first()).toContainText('demo-switch-1');
    await expect(cards.first()).toContainText('Aruba2930F');
    await expect(cards.first()).toContainText('192.168.1.1');
    await expect(cards.first()).toContainText('VLANs');
    await expect(cards.first()).toContainText('ports');
  });

  test('shows status badge', async ({ page }) => {
    await page.goto(UI_URL);
    const card = page.locator('.card').first();
    // Should show a badge (either "Not applied", "OK", etc.)
    await expect(card.locator('.badge')).toBeVisible();
  });

  test('switch card links to detail view', async ({ page }) => {
    await page.goto(UI_URL);
    await page.locator('.card').first().click();
    await expect(page).toHaveURL(/\/switch\/demo-sw-01/);
    await expect(page.locator('h2')).toContainText('demo-switch-1');
  });
});

// ============================================================================
// Switch Detail View
// ============================================================================

test.describe('Switch Detail View', () => {
  test('shows switch header info', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('h2')).toContainText('demo-switch-1');
    await expect(page.locator('body')).toContainText('Aruba2930F');
    await expect(page.locator('body')).toContainText('192.168.1.1');
  });

  test('has all 6 tabs', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('.tab')).toHaveCount(6);
    await expect(page.locator('.tabs')).toContainText('Overview');
    await expect(page.locator('.tabs')).toContainText('VLANs');
    await expect(page.locator('.tabs')).toContainText('Ports');
    await expect(page.locator('.tabs')).toContainText('Mirrors');
    await expect(page.locator('.tabs')).toContainText('SNMP');
    await expect(page.locator('.tabs')).toContainText('Config Sources');
  });

  test('has Edit button and Back to Dashboard link', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('button:has-text("Edit")')).toBeVisible();
    await expect(page.locator('a:has-text("Back to Dashboard")')).toBeVisible();
  });

  test('overview tab shows summary', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await expect(page.locator('body')).toContainText('Hostname');
    await expect(page.locator('body')).toContainText('VLANs');
    await expect(page.locator('body')).toContainText('Port Mirrors');
  });

  test('active tab is highlighted', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/vlans`);
    const vlansTab = page.locator('.tab', { hasText: 'VLANs' });
    await expect(vlansTab).toHaveClass(/active/);
  });

  test('tab navigation works via links', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('.tab', { hasText: 'Ports' }).click();
    await expect(page).toHaveURL(/\/switch\/demo-sw-01\/ports/);
    await expect(page.locator('body')).toContainText('Uplink');
  });
});

// ============================================================================
// VLANs Tab
// ============================================================================

test.describe('VLANs Tab', () => {
  test('shows VLAN table with correct data', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/vlans`);
    const table = page.locator('table');
    await expect(table).toBeVisible();
    await expect(page.locator('body')).toContainText('management');
    await expect(page.locator('body')).toContainText('users');
    await expect(page.locator('body')).toContainText('servers');
    await expect(page.locator('body')).toContainText('DHCP');
  });
});

// ============================================================================
// Ports Tab
// ============================================================================

test.describe('Ports Tab', () => {
  test('shows port table with all columns', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/ports`);
    await expect(page.locator('th')).toContainText(['Port']);
    await expect(page.locator('th')).toContainText(['Untagged VLAN']);
    await expect(page.locator('th')).toContainText(['Tagged VLANs']);
    await expect(page.locator('th')).toContainText(['Description']);
    await expect(page.locator('th')).toContainText(['Enabled']);
    await expect(page.locator('body')).toContainText('Uplink');
    await expect(page.locator('body')).toContainText('User ports');
    await expect(page.locator('body')).toContainText('Trunk');
  });
});

// ============================================================================
// Mirrors Tab
// ============================================================================

test.describe('Mirrors Tab', () => {
  test('shows mirror session data', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/mirrors`);
    await expect(page.locator('body')).toContainText('Session');
    await expect(page.locator('body')).toContainText('Source Ports');
    await expect(page.locator('body')).toContainText('Destination');
  });
});

// ============================================================================
// SNMP Tab
// ============================================================================

test.describe('SNMP Tab', () => {
  test('shows SNMP communities, trap receivers, and enabled traps', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01/snmp`);
    await expect(page.locator('body')).toContainText('public');
    await expect(page.locator('body')).toContainText('operator');
    await expect(page.locator('body')).toContainText('192.168.1.100');
    await expect(page.locator('body')).toContainText('mac-notify');
    await expect(page.locator('body')).toContainText('link-change');
  });
});

// ============================================================================
// Edit Workflow — Draft Lifecycle
// ============================================================================

test.describe('Edit Workflow - Draft', () => {
  test('Edit button starts draft and shows edit VLANs view', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await expect(page).toHaveURL(/\/edit\/vlans/);
    await expect(page.locator('body')).toContainText('Draft Mode');
    await expect(page.locator('input[name="name"]').first()).toBeVisible();
  });

  test('Discard returns to read-only without changes', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await expect(page).toHaveURL(/\/edit\/vlans/);
    await page.locator('a:has-text("Discard")').click();
    await expect(page).toHaveURL(/\/switch\/demo-sw-01$/);
    await expect(page.locator('body')).not.toContainText('Draft Mode');
  });

  test('Save dialog shows filename and priority fields', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("Save")').first().click();
    await expect(page.locator('body')).toContainText('Save Configuration Overlay');
    await expect(page.locator('input[name="filename"]')).toBeVisible();
    await expect(page.locator('input[name="filename"]')).toHaveValue(/\.yaml$/);
    await expect(page.locator('input[name="priority"]')).toBeVisible();
    await expect(page.locator('input[name="priority"]')).toHaveValue('200');
  });

  test('edit view has navigation between VLANs, Ports, Mirrors, SNMP', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    // VLANs edit view
    await expect(page).toHaveURL(/\/edit\/vlans/);
    // Navigate to Ports
    await page.locator('a:has-text("Ports")').click();
    await expect(page).toHaveURL(/\/edit\/ports/);
    // Navigate to Mirrors
    await page.locator('a:has-text("Mirrors")').click();
    await expect(page).toHaveURL(/\/edit\/mirrors/);
    // Navigate to SNMP
    await page.locator('a:has-text("SNMP")').click();
    await expect(page).toHaveURL(/\/edit\/snmp/);
    // Back to VLANs
    await page.locator('a:has-text("VLANs")').click();
    await expect(page).toHaveURL(/\/edit\/vlans/);
    // Discard to clean up
    await page.locator('a:has-text("Discard")').click();
  });
});

// ============================================================================
// Edit Workflow — VLAN CRUD
// ============================================================================

test.describe('Edit VLANs CRUD', () => {
  test('add a VLAN, verify it appears, then remove it', async ({ page }) => {
    // Start draft
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await expect(page).toHaveURL(/\/edit\/vlans/);

    // Count initial VLANs
    const initialRows = await page.locator('tbody tr').count();

    // Fill add form
    await page.locator('tfoot input[name="id"]').fill('888');
    await page.locator('tfoot input[name="name"]').fill('test-vlan-888');
    await page.locator('tfoot button:has-text("+ Add")').click();

    // Should have one more row
    await expect(page.locator('tbody tr')).toHaveCount(initialRows + 1);
    // VLAN name is inside an input field
    await expect(page.locator('input[name="name"][value="test-vlan-888"]')).toHaveCount(1);

    // Remove the new VLAN
    await page.locator('a[href*="/vlan/888/remove"]').click();

    // Should be back to initial count
    await expect(page.locator('tbody tr')).toHaveCount(initialRows);
    await expect(page.locator('input[name="name"][value="test-vlan-888"]')).toHaveCount(0);

    // Discard
    await page.locator('a:has-text("Discard")').click();
  });

  test('edit a VLAN name', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();

    // Change the first VLAN name
    const nameInput = page.locator('tbody tr').first().locator('input[name="name"]');
    await nameInput.fill('renamed-vlan');
    await page.locator('tbody tr').first().locator('button:has-text("Save")').click();

    // Should still be on edit VLANs page with updated name
    await expect(page).toHaveURL(/\/edit\/vlans/);
    await expect(page.locator('tbody tr').first().locator('input[name="name"]')).toHaveValue('renamed-vlan');

    // Discard changes
    await page.locator('a:has-text("Discard")').click();
  });
});

// ============================================================================
// Edit Workflow — Port CRUD
// ============================================================================

test.describe('Edit Ports CRUD', () => {
  test('shows all port fields as editable', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("Ports")').click();
    await expect(page).toHaveURL(/\/edit\/ports/);

    // Should have vlan input, tagged_vlans, description input, checkboxes, speed select (no mode dropdown)
    await expect(page.locator('input[name="vlan"]').first()).toBeVisible();
    await expect(page.locator('input[name="description"]').first()).toBeVisible();
    await expect(page.locator('input[name="enabled"]').first()).toBeVisible();
    await expect(page.locator('select[name="speed_duplex"]').first()).toBeVisible();

    // Should have add row in tfoot
    await expect(page.locator('tfoot input[name="port_id"]')).toBeVisible();
    await expect(page.locator('tfoot button:has-text("+ Add")')).toBeVisible();

    await page.locator('a:has-text("Discard")').click();
  });

  test('add a port, verify it appears, then remove it', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("Ports")').click();

    const initialRows = await page.locator('tbody tr').count();

    // Fill add form
    await page.locator('tfoot input[name="port_id"]').fill('99');
    await page.locator('tfoot input[name="vlan"]').fill('10');
    await page.locator('tfoot input[name="description"]').fill('Test port');
    await page.locator('tfoot button:has-text("+ Add")').click();

    await expect(page.locator('tbody tr')).toHaveCount(initialRows + 1);
    await expect(page.locator('input[name="description"][value="Test port"]')).toHaveCount(1);

    // Remove it
    await page.locator('a[href*="/port/99/remove"]').click();
    await expect(page.locator('tbody tr')).toHaveCount(initialRows);

    await page.locator('a:has-text("Discard")').click();
  });
});

// ============================================================================
// Edit Workflow — Mirror CRUD
// ============================================================================

test.describe('Edit Mirrors CRUD', () => {
  test('shows mirror edit fields', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("Mirrors")').click();
    await expect(page).toHaveURL(/\/edit\/mirrors/);

    await expect(page.locator('input[name="source_ports"]').first()).toBeVisible();
    await expect(page.locator('input[name="destination_port"]').first()).toBeVisible();
    await expect(page.locator('select[name="direction"]').first()).toBeVisible();
    await expect(page.locator('tfoot button:has-text("+ Add")')).toBeVisible();

    await page.locator('a:has-text("Discard")').click();
  });

  test('add a mirror session, then remove it', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("Mirrors")').click();

    // Should have 1 existing mirror (session "1")
    await expect(page.locator('tbody tr')).toHaveCount(1);

    await page.locator('tfoot input[name="session_id"]').fill('9');
    await page.locator('tfoot input[name="source_ports"]').fill('5,6');
    await page.locator('tfoot input[name="destination_port"]').fill('20');
    await page.locator('tfoot button:has-text("+ Add")').click();

    // Now should have 2 mirrors
    await expect(page.locator('tbody tr')).toHaveCount(2);

    // Remove the new one
    await page.locator('a[href*="/mirror/9/remove"]').click();
    await expect(page.locator('tbody tr')).toHaveCount(1);

    await page.locator('a:has-text("Discard")').click();
  });
});

// ============================================================================
// Edit Workflow — SNMP CRUD
// ============================================================================

test.describe('Edit SNMP CRUD', () => {
  test('shows SNMP communities, trap receivers, and trap checkboxes', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("SNMP")').click();
    await expect(page).toHaveURL(/\/edit\/snmp/);

    // Communities section
    await expect(page.locator('body')).toContainText('Communities');
    await expect(page.locator('body')).toContainText('public');

    // Trap receivers section
    await expect(page.locator('body')).toContainText('Trap Receivers');
    await expect(page.locator('body')).toContainText('192.168.1.100');

    // Traps section with checkboxes
    await expect(page.locator('body')).toContainText('Enabled Traps');
    await expect(page.locator('input[name="mac_notify"]')).toBeVisible();
    await expect(page.locator('input[name="link_change"]')).toBeVisible();
    await expect(page.locator('button:has-text("Update Traps")')).toBeVisible();

    await page.locator('a:has-text("Discard")').click();
  });

  test('add and remove SNMP community', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("SNMP")').click();

    // Add a community
    await page.locator('input[placeholder="Community name"]').fill('private');
    await page.locator('select[name="access"]').first().selectOption('manager');
    // Click the first "+ Add" button (communities section)
    await page.locator('button:has-text("+ Add")').first().click();

    await expect(page.locator('body')).toContainText('private');
    await expect(page.locator('body')).toContainText('manager');

    // Remove it
    await page.locator('a[href*="/snmp/community/private/remove"]').click();
    await expect(page.locator('body')).not.toContainText('private');

    await page.locator('a:has-text("Discard")').click();
  });

  test('add and remove trap receiver', async ({ page }) => {
    await page.goto(`${UI_URL}/switch/demo-sw-01`);
    await page.locator('button:has-text("Edit")').click();
    await page.locator('a:has-text("SNMP")').click();

    // Add trap receiver
    await page.locator('input[placeholder="IP address"]').fill('10.0.0.1');
    await page.locator('input[placeholder="Community"]').fill('traps');
    await page.locator('button:has-text("+ Add")').nth(1).click();

    await expect(page.locator('body')).toContainText('10.0.0.1');
    await expect(page.locator('body')).toContainText('traps');

    // Remove it
    await page.locator('a[href*="/snmp/trap-receiver/10.0.0.1/remove"]').click();
    await expect(page.locator('body')).not.toContainText('10.0.0.1');

    await page.locator('a:has-text("Discard")').click();
  });
});
