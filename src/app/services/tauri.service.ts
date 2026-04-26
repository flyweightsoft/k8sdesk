import { Injectable } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { open as dialogOpen, confirm as dialogConfirm } from '@tauri-apps/plugin-dialog';
import { readDir as fsReadDir, readTextFile as fsReadTextFile, writeTextFile as fsWriteTextFile } from '@tauri-apps/plugin-fs';
import {
  ClusterInput,
  DashboardSession,
  ExecuteRequest,
  ExecuteResponse,
  KubeconfigImport,
  RedactedCluster,
} from '../models/cluster';

export interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
  children?: DirEntry[];
  childrenLoaded?: boolean;
}

@Injectable({ providedIn: 'root' })
export class TauriService {
  clusterList(): Promise<RedactedCluster[]> {
    return invoke('cluster_list');
  }
  clusterAdd(input: ClusterInput): Promise<RedactedCluster> {
    return invoke('cluster_add', { input });
  }
  clusterUpdate(id: string, input: ClusterInput): Promise<RedactedCluster> {
    return invoke('cluster_update', { id, input });
  }
  clusterDelete(id: string): Promise<void> {
    return invoke('cluster_delete', { id });
  }
  clusterImportKubeconfig(input: KubeconfigImport): Promise<RedactedCluster> {
    return invoke('cluster_import_kubeconfig', { input });
  }
  clusterUpdateFromKubeconfig(id: string, input: KubeconfigImport): Promise<RedactedCluster> {
    return invoke('cluster_update_from_kubeconfig', { id, input });
  }
  namespaceList(clusterId: string): Promise<string[]> {
    return invoke('namespace_list', { clusterId });
  }
  dslExecute(req: ExecuteRequest): Promise<ExecuteResponse> {
    return invoke('dsl_execute', { req });
  }
  dashboardOpen(clusterId: string): Promise<DashboardSession> {
    return invoke('dashboard_open', { clusterId });
  }
  dashboardStatus(clusterId: string): Promise<DashboardSession | null> {
    return invoke('dashboard_status', { clusterId });
  }
  dashboardStop(clusterId: string): Promise<void> {
    return invoke('dashboard_stop', { clusterId });
  }
  openUrl(url: string): Promise<void> {
    return invoke('open_url', { url });
  }

  // ── File manager ─────────────────────────────────────────────────────────

  async pickFolder(): Promise<string | null> {
    const result = await dialogOpen({ directory: true, multiple: false });
    if (!result || Array.isArray(result)) return result as string | null;
    return result;
  }

  confirm(message: string, title?: string): Promise<boolean> {
    return dialogConfirm(message, { title: title ?? 'k8sdesk', kind: 'warning' });
  }

  async readDir(dirPath: string): Promise<DirEntry[]> {
    const entries = await fsReadDir(dirPath);
    // Construct full path from parent + entry name (plugin-fs DirEntry has no path field)
    const sep = dirPath.includes('\\') ? '\\' : '/';
    const base = dirPath.replace(/[/\\]$/, '');
    return entries.map(e => ({
      name: e.name ?? '',
      path: base + sep + (e.name ?? ''),
      isDir: e.isDirectory ?? false,
    }));
  }

  readTextFile(path: string): Promise<string> {
    return fsReadTextFile(path);
  }

  writeTextFile(path: string, content: string): Promise<void> {
    return fsWriteTextFile(path, content);
  }

  clusterFolderGet(clusterId: string): Promise<string | null> {
    return invoke('cluster_folder_get', { clusterId });
  }

  clusterFolderSet(clusterId: string, folderPath: string): Promise<void> {
    return invoke('cluster_folder_set', { clusterId, folderPath });
  }
}
