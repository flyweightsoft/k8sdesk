import { CommonModule } from '@angular/common';
import { Component, OnInit, OnDestroy, signal, computed, ChangeDetectionStrategy } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { TauriService } from './services/tauri.service';
import { ThemeService } from './services/theme.service';
import {
  CommandOutput,
  ConfirmationRequest,
  DashboardSession,
  Environment,
  RedactedCluster,
} from './models/cluster';
import { ClusterFormComponent } from './components/cluster-form/cluster-form.component';
import { ConfirmModalComponent } from './components/confirm-modal/confirm-modal.component';
import { FileManagerComponent } from './components/file-manager/file-manager.component';
import { TerminalComponent } from './components/terminal/terminal.component';
import { ThemeModalComponent } from './components/theme-modal/theme-modal.component';

interface PendingExec {
  command: string;
  applyBody?: string;
  request: ConfirmationRequest;
}

@Component({
  selector: 'app-root',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [
    CommonModule,
    FormsModule,
    ClusterFormComponent,
    ConfirmModalComponent,
    FileManagerComponent,
    TerminalComponent,
    ThemeModalComponent,
  ],
  templateUrl: './app.component.html',
  styleUrl: './app.component.scss',
})
export class AppComponent implements OnInit, OnDestroy {
  clusters = signal<RedactedCluster[]>([]);
  selectedClusterId = signal<string | null>(null);
  namespaces = signal<string[]>([]);
  selectedNamespace = signal<string>('default');
  loadingNs = signal(false);
  error = signal<string | null>(null);

  themeModalOpen = false;
  history = signal<HistoryEntry[]>([]);
  formOpen = signal(false);
  editing = signal<RedactedCluster | null>(null);
  pending = signal<PendingExec | null>(null);
  pendingDeleteCluster = signal<RedactedCluster | null>(null);
  dashboardLoading = signal(false);
  dashboardSession = signal<DashboardSession | null>(null);
  dashboardRunning = signal(false);

  // ── File manager ──────────────────────────────────────────────────────────
  fileManagerOpen = signal(false);
  fileManagerHeight = signal(340);
  resizing = signal(false);

  private dashboardPollTimer: ReturnType<typeof setInterval> | null = null;
  private resizeMoveHandler: ((e: MouseEvent) => void) | null = null;
  private resizeUpHandler: (() => void) | null = null;

  selectedCluster = computed(() => {
    const id = this.selectedClusterId();
    return this.clusters().find((c) => c.id === id) ?? null;
  });

  isProd = computed(() => this.selectedCluster()?.environment === 'prod');

  constructor(private tauri: TauriService, readonly theme: ThemeService) {}

  async ngOnInit(): Promise<void> {
    this.theme.init();
    await this.refreshClusters();
    this.startDashboardPoll();
  }

  ngOnDestroy(): void {
    if (this.dashboardPollTimer !== null) {
      clearInterval(this.dashboardPollTimer);
    }
    if (this.resizeMoveHandler) document.removeEventListener('mousemove', this.resizeMoveHandler);
    if (this.resizeUpHandler) document.removeEventListener('mouseup', this.resizeUpHandler);
  }

  private startDashboardPoll(): void {
    this.dashboardPollTimer = setInterval(async () => {
      const id = this.selectedClusterId();
      if (!id) { this.dashboardRunning.set(false); return; }
      try {
        const s = await this.tauri.dashboardStatus(id);
        this.dashboardRunning.set(s !== null);
      } catch {
        this.dashboardRunning.set(false);
      }
    }, 3000);
  }

  async refreshClusters(): Promise<void> {
    try {
      const list = await this.tauri.clusterList();
      this.clusters.set(list);
      if (!this.selectedClusterId() && list.length) {
        await this.selectCluster(list[0].id);
      } else if (this.selectedClusterId()) {
        const stillThere = list.find((c) => c.id === this.selectedClusterId());
        if (!stillThere) {
          this.selectedClusterId.set(null);
          this.namespaces.set([]);
        }
      }
    } catch (e) {
      this.error.set(String(e));
    }
  }

  async selectCluster(id: string): Promise<void> {
    this.selectedClusterId.set(id);
    const c = this.clusters().find((x) => x.id === id);
    this.selectedNamespace.set(c?.default_namespace ?? 'default');
    this.namespaces.set([]);
    this.loadingNs.set(true);
    try {
      const ns = await this.tauri.namespaceList(id);
      this.namespaces.set(ns);
      if (c && !ns.includes(this.selectedNamespace())) {
        this.selectedNamespace.set(ns[0] ?? c.default_namespace);
      }
    } catch (e) {
      this.appendHistory({
        kind: 'error',
        text: `failed to list namespaces: ${e}`,
      });
    } finally {
      this.loadingNs.set(false);
    }
  }

  openAdd(): void {
    this.editing.set(null);
    this.formOpen.set(true);
  }

  openEdit(c: RedactedCluster): void {
    this.editing.set(c);
    this.formOpen.set(true);
  }

  openDeleteConfirm(c: RedactedCluster): void {
    this.pendingDeleteCluster.set(c);
  }

  async openDashboard(): Promise<void> {
    const id = this.selectedClusterId();
    if (!id) return;
    this.dashboardLoading.set(true);
    this.dashboardSession.set(null);
    try {
      const session = await this.tauri.dashboardOpen(id);
      this.dashboardSession.set(session);
      this.dashboardRunning.set(true);
    } catch (e) {
      this.error.set(`Dashboard: ${e}`);
    } finally {
      this.dashboardLoading.set(false);
    }
  }

  async stopDashboard(): Promise<void> {
    const id = this.selectedClusterId();
    if (!id) return;
    try {
      await this.tauri.dashboardStop(id);
      this.dashboardSession.set(null);
      this.dashboardRunning.set(false);
    } catch (e) {
      this.error.set(`Dashboard stop: ${e}`);
    }
  }

  openUrl(url: string): void {
    this.tauri.openUrl(url).catch(e => this.error.set(`${e}`));
  }

  async confirmDelete(): Promise<void> {
    const c = this.pendingDeleteCluster();
    if (!c) return;
    this.pendingDeleteCluster.set(null);
    try {
      await this.tauri.clusterDelete(c.id);
      await this.refreshClusters();
    } catch (e) {
      this.error.set(String(e));
    }
  }

  cancelDelete(): void {
    this.pendingDeleteCluster.set(null);
  }

  async onFormSaved(): Promise<void> {
    this.formOpen.set(false);
    await this.refreshClusters();
  }

  async runCommand(payload: { command: string; applyBody?: string }): Promise<void> {
    const clusterId = this.selectedClusterId();
    if (!clusterId) {
      this.appendHistory({ kind: 'error', text: 'select a cluster first' });
      return;
    }
    this.appendHistory({
      kind: 'input',
      cluster: this.selectedCluster()?.name ?? '',
      namespace: this.selectedNamespace(),
      text: payload.command,
    });
    await this.executeInner(payload.command, payload.applyBody, undefined);
  }

  private async executeInner(
    command: string,
    applyBody: string | undefined,
    confirmation: string | undefined,
  ): Promise<void> {
    const clusterId = this.selectedClusterId();
    if (!clusterId) return;
    try {
      const res = await this.tauri.dslExecute({
        cluster_id: clusterId,
        namespace: this.selectedNamespace(),
        command,
        apply_body: applyBody,
        confirmation,
      });
      if (res.kind === 'needs_confirmation') {
        this.pending.set({ command, applyBody, request: res.request });
        return;
      }
      this.appendHistory({ kind: 'output', output: res.output, severity: res.severity });
    } catch (e) {
      this.appendHistory({ kind: 'error', text: String(e) });
    }
  }

  async onConfirm(token: string): Promise<void> {
    const p = this.pending();
    if (!p) return;
    this.pending.set(null);
    await this.executeInner(p.command, p.applyBody, token);
  }

  onCancelConfirm(): void {
    this.pending.set(null);
    this.appendHistory({ kind: 'error', text: 'confirmation cancelled' });
  }

  private appendHistory(e: HistoryEntry): void {
    const next = [...this.history(), e];
    // Cap at 10k entries to keep memory bounded.
    if (next.length > 10_000) next.splice(0, next.length - 10_000);
    this.history.set(next);
  }

  clearHistory(): void {
    this.history.set([]);
  }

  /** Remove the input at `inputIdx` plus all immediately following output/error entries. */
  clearGroup(inputIdx: number): void {
    const h = this.history();
    let end = inputIdx + 1;
    while (end < h.length && h[end].kind !== 'input') end++;
    this.history.set([...h.slice(0, inputIdx), ...h.slice(end)]);
  }

  envClass(env: Environment): string {
    return `env-${env}`;
  }

  // ── File manager integration ───────────────────────────────────────────────

  toggleFileManager(): void {
    this.fileManagerOpen.update(v => !v);
  }

  applyFromFileManager(payload: { path: string; content: string }): void {
    const clusterId = this.selectedClusterId();
    if (!clusterId) {
      this.appendHistory({ kind: 'error', text: 'select a cluster before applying' });
      return;
    }
    const fileName = payload.path.replace(/\\/g, '/').split('/').pop() ?? 'file';
    // Generate a paste id (matches the terminal's apply flow).
    const pasteId = `fm-${Date.now().toString(36)}`;
    const command = `apply paste:${pasteId}`;
    this.appendHistory({
      kind: 'input',
      cluster: this.selectedCluster()?.name ?? '',
      namespace: this.selectedNamespace(),
      text: `kubectl apply -f ${fileName}`,
    });
    this.executeInner(command, payload.content, undefined);
  }

  onResizeStart(event: MouseEvent): void {
    event.preventDefault();
    this.resizing.set(true);
    const startY = event.clientY;
    const startH = this.fileManagerHeight();

    this.resizeMoveHandler = (e: MouseEvent) => {
      const newH = Math.max(160, Math.min(600, startH + (e.clientY - startY)));
      this.fileManagerHeight.set(newH);
    };
    this.resizeUpHandler = () => {
      this.resizing.set(false);
      if (this.resizeMoveHandler) document.removeEventListener('mousemove', this.resizeMoveHandler);
      if (this.resizeUpHandler) document.removeEventListener('mouseup', this.resizeUpHandler);
      this.resizeMoveHandler = null;
      this.resizeUpHandler = null;
    };
    document.addEventListener('mousemove', this.resizeMoveHandler);
    document.addEventListener('mouseup', this.resizeUpHandler);
  }
}

export type HistoryEntry =
  | { kind: 'input'; cluster: string; namespace: string; text: string }
  | { kind: 'output'; severity: 'safe' | 'destructive'; output: CommandOutput }
  | { kind: 'error'; text: string };
