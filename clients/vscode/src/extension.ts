import * as vscode from 'vscode';
import { HearthApi, Identity, TestResults } from './api';
import { HearthFsProvider, Target } from './fsProvider';
import { Node, ProgramsTree } from './tree';

let identity: Identity | undefined;

export function activate(context: vscode.ExtensionContext) {
  const api = new HearthApi();
  const fs = new HearthFsProvider(api);
  const tree = new ProgramsTree(api, fs);
  const output = vscode.window.createOutputChannel('Hearth MUD');

  // Read-only provider for historical version bodies (diff/view).
  const versionProvider = new (class implements vscode.TextDocumentContentProvider {
    async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
      // hearth-version:/<enc ref>/<version>?name=<enc name>
      const parts = uri.path.replace(/^\//, '').split('/');
      const refId = decodeURIComponent(parts[0]);
      const version = Number(parts[1]);
      const q = new URLSearchParams(uri.query);
      const name = q.get('name') ? decodeURIComponent(q.get('name')!) : undefined;
      try {
        const res = await api.getScriptVersion(refId, version, name);
        return res.source;
      } catch (e: any) {
        return `-- could not fetch version ${version}: ${e?.message ?? e}\n`;
      }
    }
  })();

  context.subscriptions.push(
    vscode.workspace.registerFileSystemProvider(HearthFsProvider.scheme, fs, { isCaseSensitive: true }),
    vscode.workspace.registerTextDocumentContentProvider('hearth-version', versionProvider),
    vscode.window.registerTreeDataProvider('hearthObjects', tree),
    output,
  );

  const activeTarget = (): { uri: vscode.Uri; t: Target } | undefined => {
    const uri = vscode.window.activeTextEditor?.document.uri;
    if (!uri || uri.scheme !== HearthFsProvider.scheme) return undefined;
    const t = fs.targetOf(uri);
    return t ? { uri, t } : undefined;
  };
  const nameOf = (t: Target) => (t.kind === 'lib' ? t.name : undefined);

  async function publish() {
    const cur = activeTarget();
    if (!cur) return void vscode.window.showWarningMessage('Open a Hearth script to publish.');
    const editor = vscode.window.activeTextEditor!;
    const source = editor.document.getText();
    const check = await api.checkProgram(source).catch(() => ({ valid: true }));
    if (!check.valid) return void vscode.window.showErrorMessage(`Luau syntax error — fix before publishing.`);
    const res = await api.publish(cur.t.refId, source, fs.baseVersionOf(cur.uri), nameOf(cur.t));
    if (res.kind === 'ok') {
      fs.setBaseVersion(cur.uri, res.version);
      fs.markPublished(cur.uri);
      tree.refresh();
      vscode.window.showInformationMessage(
        res.merged_from
          ? `Published v${res.version} (merged across v${res.merged_from}).`
          : `Published v${res.version}.`,
      );
    } else if (res.kind === 'conflict') {
      const choice = await vscode.window.showWarningMessage(
        `Conflict: the server moved to v${res.current_version} since you opened this. Reconcile your changes.`,
        { modal: true },
        'Open Diff (server ↔ mine)',
        'Overwrite with mine',
      );
      if (choice === 'Overwrite with mine') {
        fs.setBaseVersion(cur.uri, res.current_version);
        return publish();
      }
      if (choice === 'Open Diff (server ↔ mine)') {
        const theirs = await vscode.workspace.openTextDocument({ content: res.theirs, language: 'luau' });
        // Rebasing onto the server version: a later Publish now applies cleanly.
        fs.setBaseVersion(cur.uri, res.current_version);
        await vscode.commands.executeCommand(
          'vscode.diff',
          theirs.uri,
          cur.uri,
          `Server v${res.current_version} ↔ your changes`,
        );
        vscode.window.showInformationMessage('Reconcile in the right pane, then Publish again.');
      }
    } else {
      vscode.window.showErrorMessage(`Publish failed: ${res.message}`);
    }
  }

  async function lock(node?: Node) {
    const ref = targetFromNodeOrActive(node);
    if (!ref) return;
    try {
      const l = await api.lockScript(ref.refId, ref.name);
      vscode.window.showInformationMessage(`Edit lock claimed (until ${new Date(l.expires_at * 1000).toLocaleTimeString()}).`);
      tree.refresh();
    } catch (e: any) {
      vscode.window.showWarningMessage(`Could not claim lock: ${e?.message ?? e}`);
    }
  }

  async function unlock(node?: Node) {
    const ref = targetFromNodeOrActive(node);
    if (!ref) return;
    try {
      await api.unlockScript(ref.refId, ref.name);
      vscode.window.showInformationMessage('Edit lock released.');
      tree.refresh();
    } catch (e: any) {
      vscode.window.showErrorMessage(`Could not release lock: ${e?.message ?? e}`);
    }
  }

  async function history() {
    const cur = activeTarget();
    if (!cur) return void vscode.window.showWarningMessage('Open a Hearth script to see its history.');
    const name = nameOf(cur.t);
    const { versions } = await api.listScriptVersions(cur.t.refId, name);
    if (versions.length === 0) return void vscode.window.showInformationMessage('No version history yet.');
    const pick = await vscode.window.showQuickPick(
      versions.map((v) => ({
        label: `v${v.version} · ${v.author_name}`,
        description: v.merged_from ? `merged across v${v.merged_from}` : v.origin,
        detail: new Date(v.created_at * 1000).toLocaleString(),
        version: v.version,
      })),
      { placeHolder: 'Pick a version' },
    );
    if (!pick) return;
    const action = await vscode.window.showQuickPick(['Diff against current', 'View', 'Revert to this'], {
      placeHolder: `v${pick.version}`,
    });
    const versionUri = vscode.Uri.parse(
      `hearth-version:/${encodeURIComponent(cur.t.refId)}/${pick.version}` +
        (name ? `?name=${encodeURIComponent(name)}` : ''),
    );
    if (action === 'View') {
      await vscode.window.showTextDocument(versionUri, { preview: true });
    } else if (action === 'Diff against current') {
      await vscode.commands.executeCommand('vscode.diff', versionUri, cur.uri, `v${pick.version} ↔ current`);
    } else if (action === 'Revert to this') {
      const res = await api.revertScript(cur.t.refId, pick.version, name);
      await fs.open(cur.t); // reload the buffer from the new current
      vscode.window.showInformationMessage(`Reverted — new version v${res.version}.`);
      tree.refresh();
    }
  }

  function targetFromNodeOrActive(node?: Node): { refId: string; name?: string } | undefined {
    if (node?.kind === 'object') return { refId: node.obj.ref_id };
    const cur = activeTarget();
    if (cur) return { refId: cur.t.refId, name: nameOf(cur.t) };
    vscode.window.showWarningMessage('Select a Hearth script first.');
    return undefined;
  }

  context.subscriptions.push(
    vscode.commands.registerCommand('hearth.refresh', () => tree.refresh()),
    vscode.commands.registerCommand('hearth.openObject', (refId: string) => fs.open({ kind: 'obj', refId })),
    vscode.commands.registerCommand('hearth.openLib', (refId: string, name: string) =>
      fs.open({ kind: 'lib', refId, name }),
    ),
    vscode.commands.registerCommand('hearth.publish', publish),
    vscode.commands.registerCommand('hearth.lock', lock),
    vscode.commands.registerCommand('hearth.unlock', unlock),
    vscode.commands.registerCommand('hearth.history', history),

    vscode.commands.registerCommand('hearth.connect', async () => {
      if (!api.token) {
        const open = 'Open Settings';
        const choice = await vscode.window.showWarningMessage(
          'No Hearth API token set. Mint one in-game with `@token create <label>`, then paste it into settings.',
          open,
        );
        if (choice === open) vscode.commands.executeCommand('workbench.action.openSettings', 'hearth.token');
        return;
      }
      try {
        identity = await api.me();
        tree.refresh();
        vscode.window.showInformationMessage(`Connected to Hearth as ${identity.username} (${api.serverUrl}).`);
      } catch (e: any) {
        vscode.window.showErrorMessage(`Hearth connection failed: ${e?.message ?? e}`);
      }
    }),

    vscode.commands.registerCommand('hearth.runTests', async (node?: Node) => {
      const refId = node?.kind === 'object' ? node.obj.ref_id : activeTarget()?.t.refId;
      if (!refId) return void vscode.window.showWarningMessage('Run tests from an object in the Hearth view.');
      try {
        showTestResults(output, refId, await api.runTests(refId));
      } catch (e: any) {
        vscode.window.showErrorMessage(`Run tests failed: ${e?.message ?? e}`);
      }
    }),
  );

  if (api.token) vscode.commands.executeCommand('hearth.connect');
}

export function deactivate() {}

function showTestResults(output: vscode.OutputChannel, refId: string, r: TestResults) {
  output.show(true);
  output.appendLine(`\n=== Tests for ${refId} ===`);
  for (const file of r.files) {
    if (file.error) {
      output.appendLine(`  ${file.file}: ERROR ${file.error}`);
      continue;
    }
    for (const t of file.tests) {
      output.appendLine(`  ${t.passed ? '✓' : '✗'} ${t.name}${t.error ? ` — ${t.error}` : ''}`);
    }
  }
  output.appendLine(`--- ${r.passed} passed, ${r.failed} failed ---`);
  if (r.failed > 0) vscode.window.showErrorMessage(`Hearth tests: ${r.failed} failed, ${r.passed} passed.`);
  else vscode.window.showInformationMessage(`Hearth tests: all ${r.passed} passed.`);
}
