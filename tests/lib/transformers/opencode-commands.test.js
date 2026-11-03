import { describe, test, expect, beforeEach, afterEach } from 'bun:test';
import fs from 'fs';
import path from 'path';
import { createTransformer } from '../../../scripts/lib/transformers/factory.js';
import { PROVIDERS } from '../../../scripts/lib/transformers/providers.js';

const config = PROVIDERS.opencode;
const transform = createTransformer(config);

const TEST_DIR = path.join(process.cwd(), 'test-tmp-opencode-commands');
const COMMANDS_DIR = path.join(TEST_DIR, `${config.provider}/${config.configDir}/commands`);
const COMMAND_PATH = path.join(COMMANDS_DIR, 'impeccable.md');
const DIRECT_COMMANDS = ['critique', 'audit', 'polish'];
const shortcutPath = (command) => path.join(COMMANDS_DIR, `impeccable-${command}.md`);
const CRITIQUE_PATH = shortcutPath('critique');

const CRITIQUE_DESCRIPTION = 'UX design review with heuristic scoring.';
const AUDIT_DESCRIPTION = 'Technical quality checks.';
const POLISH_DESCRIPTION = 'Final quality pass.';

const SAMPLE_SKILL = {
  name: 'impeccable',
  description: 'Use when the user wants to design, redesign, shape, critique, audit, polish, clarify, distill, harden, optimize, adapt, animate, colorize, extract, or otherwise improve a frontend interface.',
  body: '# Impeccable\n\nSkill body here.',
  references: [],
  scripts: [
    {
      name: 'command-metadata.json',
      content: JSON.stringify({
        critique: { description: CRITIQUE_DESCRIPTION, argumentHint: '[target]' },
        audit: { description: AUDIT_DESCRIPTION, argumentHint: '[target]' },
        polish: { description: POLISH_DESCRIPTION, argumentHint: '[target]' },
      }),
    },
  ],
  agents: [],
};

beforeEach(() => {
  if (fs.existsSync(TEST_DIR)) {
    fs.rmSync(TEST_DIR, { recursive: true, force: true });
  }
});

afterEach(() => {
  if (fs.existsSync(TEST_DIR)) {
    fs.rmSync(TEST_DIR, { recursive: true, force: true });
  }
});

describe('opencode commands bridge', () => {
  test('emits .opencode/commands/impeccable.md alongside the skill', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    expect(fs.existsSync(COMMAND_PATH)).toBe(true);
  });

  test('command frontmatter uses only fields OpenCode recognises', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const content = fs.readFileSync(COMMAND_PATH, 'utf-8');
    const fm = content.match(/^---\n([\s\S]*?)\n---/);
    expect(fm).not.toBeNull();
    const lines = fm[1].split('\n').map(l => l.trim()).filter(Boolean);
    const keys = lines.map(l => l.split(':')[0]);
    // OpenCode only recognises: description, agent, model, subagent, variant, and
    // the deprecated alias subtask (per opencode/packages/core/src/v1/config/command.ts).
    const allowed = new Set(['description', 'agent', 'model', 'subagent', 'variant', 'subtask']);
    for (const key of keys) {
      expect(allowed.has(key)).toBe(true);
    }
  });

  test('command description mirrors the skill description exactly', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const content = fs.readFileSync(COMMAND_PATH, 'utf-8');
    const fm = content.match(/^---\n([\s\S]*?)\n---/)[1];
    const line = fm.split('\n').find(l => l.startsWith('description:'));
    const value = line.slice('description:'.length).trim().replace(/^"(.*)"$/, '$1');
    expect(value).toBe(SAMPLE_SKILL.description);
  });

  test('command body delegates to the impeccable skill', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const content = fs.readFileSync(COMMAND_PATH, 'utf-8');
    const body = content.replace(/^---\n[\s\S]*?\n---\n/, '');
    expect(body).toContain('skill({');
    expect(body).toContain("name: \"impeccable\"");
    expect(body).toContain('Setup');
    expect(body).toContain('Commands');
    expect(body).toContain('$ARGUMENTS');
  });

  test('command sets no agent and runs in the current session', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const content = fs.readFileSync(COMMAND_PATH, 'utf-8');
    // Only `description`: the command runs under the agent and permission set
    // the user is already working with. A child session would send the
    // playbook's report somewhere else and hand the parent one line back.
    expect(content).not.toMatch(/^agent:/m);
    expect(content).not.toMatch(/^subagent:/m);
    expect(content).not.toMatch(/^subtask:/m);
  });

  test('does not emit Claude-only frontmatter fields on the command', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const content = fs.readFileSync(COMMAND_PATH, 'utf-8');
    const fm = content.match(/^---\n([\s\S]*?)\n---/)[1];
    expect(fm).not.toMatch(/^version:/m);
    expect(fm).not.toMatch(/^user-invocable:/m);
    expect(fm).not.toMatch(/^argument-hint:/m);
    expect(fm).not.toMatch(/^allowed-tools:/m);
  });

  test('emits no command when the skill is empty', () => {
    transform([], TEST_DIR);
    expect(fs.existsSync(path.dirname(COMMAND_PATH))).toBe(false);
  });

  test('keeps emitting the skill alongside the command', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const skillPath = path.join(
      TEST_DIR,
      `${config.provider}/${config.configDir}/skills/impeccable/SKILL.md`,
    );
    expect(fs.existsSync(skillPath)).toBe(true);
    expect(fs.existsSync(COMMAND_PATH)).toBe(true);
  });

  test('emits a direct shortcut for critique, audit and polish beside the bridge', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    for (const command of DIRECT_COMMANDS) {
      expect(fs.existsSync(shortcutPath(command))).toBe(true);
    }
  });

  test('each shortcut is the bridge with its own command word in front', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    for (const command of DIRECT_COMMANDS) {
      const body = fs
        .readFileSync(shortcutPath(command), 'utf-8')
        .replace(/^---\n[\s\S]*?\n---\n/, '');
      // The skill's own routing turns the named command into its playbook.
      expect(body).toContain('skill({');
      expect(body).toContain('name: "impeccable"');
      expect(body).toContain(`\`${command}\``);
      expect(body).toContain('$ARGUMENTS');
      for (const other of DIRECT_COMMANDS.filter((c) => c !== command)) {
        expect(body).not.toContain(`\`${other}\``);
      }
    }
  });

  test('shortcuts keep the caller\'s agent, permissions and session', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    for (const command of DIRECT_COMMANDS) {
      const content = fs.readFileSync(shortcutPath(command), 'utf-8');
      expect(content).not.toMatch(/^agent:/m);
      expect(content).not.toMatch(/^subagent:/m);
      expect(content).not.toMatch(/^subtask:/m);
    }
  });

  test('shortcut descriptions come from command metadata', () => {
    transform([SAMPLE_SKILL], TEST_DIR);
    const expected = { critique: CRITIQUE_DESCRIPTION, audit: AUDIT_DESCRIPTION, polish: POLISH_DESCRIPTION };
    for (const [command, description] of Object.entries(expected)) {
      const fm = fs.readFileSync(shortcutPath(command), 'utf-8').match(/^---\n([\s\S]*?)\n---/)[1];
      const line = fm.split('\n').find((l) => l.startsWith('description:'));
      const value = line.slice('description:'.length).trim().replace(/^"(.*)"$/, '$1');
      expect(value).toBe(description);
    }
  });

  test('falls back to a generic description without command metadata', () => {
    transform([{ ...SAMPLE_SKILL, scripts: [] }], TEST_DIR);
    for (const command of DIRECT_COMMANDS) {
      expect(fs.existsSync(shortcutPath(command))).toBe(true);
      expect(fs.readFileSync(shortcutPath(command), 'utf-8')).toContain(
        `Runs Impeccable's ${command} workflow`,
      );
    }
  });
});
