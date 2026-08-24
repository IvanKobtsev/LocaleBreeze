import * as path from 'node:path';
import * as fs from 'node:fs';
import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions, Trace } from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const settings = vscode.workspace.getConfiguration('localeBreeze');
  const configured = settings.get<string>('server.path', '').trim();
  const executable = configured || bundledServer(context);
  if (!fs.existsSync(executable)) {
    void vscode.window.showErrorMessage(`LocaleBreeze server was not found at ${executable}. Configure localeBreeze.server.path.`);
    return;
  }
  if (process.platform !== 'win32') fs.chmodSync(executable, 0o755);

  const args = ['lsp', '--stdio'];
  const config = resolveConfig(settings.get<string>('configPath', '').trim());
  if (config) args.push('--config', config);
  const serverOptions: ServerOptions = { command: executable, args, options: { cwd: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath } };
  const configWatcher = vscode.workspace.createFileSystemWatcher('**/locale-breeze.json');
  context.subscriptions.push(configWatcher);
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'typescript' }, { scheme: 'file', language: 'typescriptreact' },
      { scheme: 'file', language: 'javascript' }, { scheme: 'file', language: 'javascriptreact' },
      { scheme: 'file', language: 'json' }
    ],
    synchronize: { configurationSection: 'localeBreeze', fileEvents: configWatcher },
    outputChannelName: 'LocaleBreeze'
  };
  client = new LanguageClient('localeBreeze', 'LocaleBreeze', serverOptions, clientOptions);
  const trace = settings.get<string>('server.trace', 'off');
  client.setTrace(trace === 'verbose' ? Trace.Verbose : trace === 'messages' ? Trace.Messages : Trace.Off);
  await client.start();
  context.subscriptions.push({ dispose: () => void client?.stop() });
}

export async function deactivate(): Promise<void> { await client?.stop(); }

function bundledServer(context: vscode.ExtensionContext): string {
  const platform = `${process.platform}-${process.arch}`;
  const name = process.platform === 'win32' ? 'locale-breeze.exe' : 'locale-breeze';
  return context.asAbsolutePath(path.join('bin', platform, name));
}

function resolveConfig(value: string): string | undefined {
  if (!value) return undefined;
  if (path.isAbsolute(value)) return value;
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  return root ? path.join(root, value) : value;
}
