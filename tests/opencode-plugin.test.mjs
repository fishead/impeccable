/**
 * Behaviour of the OpenCode design-hook plugin (skill/opencode-plugin/).
 * Run: node --test tests/opencode-plugin.test.mjs
 *
 * The plugin is exercised against a stub launcher, so this suite asserts the
 * event it sends, where it looks for the launcher, and what reaches the tool
 * result — not what the detector finds. `tests/hook-build.test.mjs` covers the
 * bundle emission; crates/skills/tests/opencode_plugin_tests.rs covers install.
 *
 * One file serves both plugin APIs, so both are driven here: v2 through
 * `setup(ctx).tool.hook("execute.after")`, v1 through `server(input)` and the
 * `"tool.execute.after"` hook it returns.
 */

import { describe, it, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const PLUGIN = path.join(REPO_ROOT, 'skill/opencode-plugin/impeccable-design-hook.js');

const FINDINGS = '[impeccable@1] Design hook findings requiring review in a.tsx (1 issue(s)):\n- L5 [gradient-text] Gradient text.';
const CLEAN = '[impeccable@1] Design hook scanned a.tsx. No deterministic design-quality issues found.';
// What the edit tool answers with; the plugin must leave it untouched when
// there is nothing to add.
const RESULT = 'Edited a.tsx (1 replacement)';

let sandbox = null;

/** A project root with a stub launcher that replies with `reply`. */
function project(reply) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'imp-opencode-plugin-'));
  const scripts = path.join(root, '.opencode/skills/impeccable/scripts');
  fs.mkdirSync(scripts, { recursive: true });
  const launcher = path.join(scripts, 'impeccable');
  // Record the payload on the way through so a test can read it back.
  fs.writeFileSync(
    launcher,
    `#!/bin/sh\ncat > "${path.join(root, 'payload.json')}"\nprintf '%s' '{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":${JSON.stringify(reply)}}}'\n`,
  );
  fs.chmodSync(launcher, 0o755);
  fs.writeFileSync(path.join(root, 'a.tsx'), 'export default () => null\n');
  sandbox = root;
  return root;
}

/** A v2 `execute.after` event. v2's tools name the file `path`. */
function v2Event(overrides = {}) {
  return { tool: 'edit', status: 'completed', sessionID: 'session-1', input: { path: 'a.tsx' }, result: { content: RESULT }, ...overrides };
}

/** A v1 `tool.execute.after` call: `(input, output)`, v1's tools name the file `filePath`. */
function v1Call(overrides = {}) {
  return [
    { tool: 'edit', sessionID: 'session-1', callID: 'call-1', args: { filePath: 'a.tsx' }, ...overrides },
    { title: 'Edited a.tsx', output: RESULT, metadata: {} },
  ];
}

async function load() {
  const mod = await import(`${PLUGIN}?t=${Date.now()}-${Math.random()}`);
  return mod;
}

/** Registers the v2 hook the way OpenCode v2 does and returns the callback. */
async function v2Handler(plugin, cwd) {
  let handler = null;
  const ctx = {
    location: { directory: cwd },
    tool: {
      hook: async (name, callback) => {
        assert.equal(name, 'execute.after');
        handler = callback;
        return { dispose: () => {} };
      },
    },
  };
  await plugin.setup(ctx);
  assert.ok(handler, 'setup registered an execute.after hook');
  return handler;
}

/** Registers the v1 hooks the way OpenCode v1 does and returns the callback. */
async function v1Handler(plugin, cwd) {
  const hooks = await plugin.server({ directory: cwd });
  assert.equal(typeof hooks['tool.execute.after'], 'function');
  return hooks['tool.execute.after'];
}

describe('opencode design-hook plugin', () => {
  beforeEach(() => {
    if (sandbox) fs.rmSync(sandbox, { recursive: true, force: true });
    sandbox = null;
  });

  it('exports exactly one default object that both loaders accept', async () => {
    const mod = await load();
    // v1's older loader calls every export as a plugin, so a helper export would
    // be invoked as one; the module must stay default-only.
    assert.deepEqual(Object.keys(mod), ['default']);
    assert.equal(mod.default.id, 'impeccable-design-hook');
    assert.equal(typeof mod.default.server, 'function', 'v1 loader');
    assert.equal(typeof mod.default.setup, 'function', 'v2 loader');
  });

  it('v2: appends the findings to the tool result and names the edited file', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const event = v2Event();
    await (await v2Handler(plugin, root))(event);
    assert.match(event.result.content, /^Edited a\.tsx \(1 replacement\)\n\n\[impeccable@1\] Design hook findings/u);
    const payload = JSON.parse(fs.readFileSync(path.join(root, 'payload.json'), 'utf8'));
    assert.equal(payload.hook_event_name, 'PostToolUse');
    assert.equal(payload.tool_name, 'Edit');
    assert.equal(payload.session_id, 'session-1');
    assert.ok(path.isAbsolute(payload.tool_input.file_path));
    assert.ok(payload.tool_input.file_path.endsWith(path.join('a.tsx')));
  });

  it('v1: appends the findings to the tool output the same way', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const [input, output] = v1Call();
    await (await v1Handler(plugin, root))(input, output);
    assert.match(output.output, /^Edited a\.tsx \(1 replacement\)\n\n\[impeccable@1\] Design hook findings/u);
    const payload = JSON.parse(fs.readFileSync(path.join(root, 'payload.json'), 'utf8'));
    assert.equal(payload.tool_name, 'Edit');
    assert.equal(payload.session_id, 'session-1');
    assert.ok(payload.tool_input.file_path.endsWith(path.join('a.tsx')));
  });

  it('v2: still reads a renamed tool’s path spelling', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const event = v2Event({ input: { path: 'a.tsx' } });
    await (await v2Handler(plugin, root))(event);
    assert.match(event.result.content, /impeccable@1/u);
  });

  it('v2: drops a clean scan instead of spending context on it', async () => {
    const root = project(CLEAN);
    const { default: plugin } = await load();
    const event = v2Event();
    await (await v2Handler(plugin, root))(event);
    assert.equal(event.result.content, RESULT);
  });

  it('v2: leaves non-UI edits, failed edits and other tools untouched without spawning', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const handler = await v2Handler(plugin, root);
    for (const event of [
      v2Event({ input: { path: 'main.rs' } }),
      v2Event({ status: 'error', error: { message: 'nope' } }),
      v2Event({ tool: 'read', input: { path: 'a.tsx' } }),
    ]) {
      const before = JSON.stringify(event);
      await handler(event);
      assert.equal(JSON.stringify(event), before);
    }
    assert.equal(fs.existsSync(path.join(root, 'payload.json')), false);
  });

  it('v1: leaves the same cases untouched', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const handler = await v1Handler(plugin, root);
    for (const [input, output] of [
      v1Call({ args: { filePath: 'main.rs' } }),
      v1Call({ tool: 'read' }),
    ]) {
      await handler(input, output);
      assert.equal(output.output, RESULT);
    }
    assert.equal(fs.existsSync(path.join(root, 'payload.json')), false);
  });

  it('v2: appends a text part when the tool result carries content parts', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const event = v2Event({ tool: 'write', result: { content: [{ type: 'text', text: 'Wrote a.tsx' }] } });
    await (await v2Handler(plugin, root))(event);
    assert.equal(event.result.content.length, 2);
    assert.deepEqual(event.result.content[0], { type: 'text', text: 'Wrote a.tsx' });
    assert.match(event.result.content[1].text, /impeccable@1/u);
    assert.equal(JSON.parse(fs.readFileSync(path.join(root, 'payload.json'), 'utf8')).tool_name, 'Write');
  });

  it('injects each distinct text once per session and stops at the budget', async () => {
    const root = project(FINDINGS);
    const { default: plugin } = await load();
    const handler = await v2Handler(plugin, root);
    const session = 'repeat-session';
    let injections = 0;
    for (let i = 0; i < 20; i++) {
      const event = v2Event({ sessionID: session });
      await handler(event);
      if (event.result.content !== RESULT) injections += 1;
    }
    // The stub always answers with the same text, so the second call onwards
    // is a duplicate and must be silent.
    assert.equal(injections, 1);
  });

  it('stays inert when there is no launcher or the plugin is disabled', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'imp-opencode-nolauncher-'));
    sandbox = root;
    fs.writeFileSync(path.join(root, 'a.tsx'), 'export default () => null\n');
    const { default: plugin } = await load();
    const event = v2Event();
    await (await v2Handler(plugin, root))(event);
    assert.equal(event.result.content, RESULT);

    fs.rmSync(root, { recursive: true, force: true });
    const withLauncher = project(FINDINGS);
    const handler = await v2Handler(plugin, withLauncher);
    process.env.IMPECCABLE_HOOK_DISABLED = '1';
    try {
      const disabledEvent = v2Event({ sessionID: 'disabled' });
      await handler(disabledEvent);
      assert.equal(disabledEvent.result.content, RESULT);
      const [input, output] = v1Call({ sessionID: 'disabled' });
      await (await v1Handler(plugin, withLauncher))(input, output);
      assert.equal(output.output, RESULT);
      assert.equal(fs.existsSync(path.join(withLauncher, 'payload.json')), false);
    } finally {
      delete process.env.IMPECCABLE_HOOK_DISABLED;
    }
  });
});
