import assert from 'node:assert/strict';
import test from 'node:test';

import { copyText } from './copy-command.js';

function fakeDocument(copyResult = true) {
  const state = {
    appended: false,
    removed: false,
    selected: false,
    value: null
  };
  const textArea = {
    style: {},
    setAttribute() {},
    select() {
      state.selected = true;
    },
    remove() {
      state.removed = true;
    },
    set value(value) {
      state.value = value;
    }
  };
  return {
    state,
    document: {
      body: {
        appendChild(node) {
          assert.equal(node, textArea);
          state.appended = true;
        }
      },
      createElement(tag) {
        assert.equal(tag, 'textarea');
        return textArea;
      },
      execCommand(command) {
        assert.equal(command, 'copy');
        return copyResult;
      }
    }
  };
}

test('uses the modern clipboard API without creating a textarea', async () => {
  const calls = [];
  const navigatorObject = {
    clipboard: {
      async writeText(text) {
        calls.push(text);
      }
    }
  };

  await copyText('command', navigatorObject, null);

  assert.deepEqual(calls, ['command']);
});

test('falls back when clipboard.writeText rejects', async () => {
  const fixture = fakeDocument();
  const navigatorObject = {
    clipboard: {
      async writeText() {
        throw new Error('permission denied');
      }
    }
  };

  await copyText('fallback command', navigatorObject, fixture.document);

  assert.deepEqual(fixture.state, {
    appended: true,
    removed: true,
    selected: true,
    value: 'fallback command'
  });
});

test('reports a rejected legacy copy and always removes the textarea', async () => {
  const fixture = fakeDocument(false);

  await assert.rejects(
    () => copyText('command', {}, fixture.document),
    /copy command was rejected/
  );
  assert.equal(fixture.state.removed, true);
});
