import { CommonModule } from '@angular/common';
import {
  ChangeDetectionStrategy,
  Component,
  EventEmitter,
  Input,
  OnInit,
  Output,
  signal,
} from '@angular/core';
import { FormsModule } from '@angular/forms';
import {
  Auth,
  ClusterInput,
  Environment,
  RedactedCluster,
} from '../../models/cluster';
import { TauriService } from '../../services/tauri.service';

type Mode = 'manual' | 'kubeconfig';
type AuthKind = 'bearer_token' | 'client_cert';

@Component({
  selector: 'app-cluster-form',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [CommonModule, FormsModule],
  templateUrl: './cluster-form.component.html',
  styleUrl: './cluster-form.component.scss',
})
export class ClusterFormComponent implements OnInit {
  @Input() editing: RedactedCluster | null = null;
  @Output() saved = new EventEmitter<void>();
  @Output() cancelled = new EventEmitter<void>();

  mode = signal<Mode>('manual');

  // form state
  name = signal('');
  environment = signal<Environment>('dev');
  apiServer = signal('');
  caPem = signal('');
  insecure = signal(false);
  defaultNamespace = signal('default');
  authKind = signal<AuthKind>('bearer_token');
  token = signal('');
  certPem = signal('');
  keyPem = signal('');

  // kubeconfig paste
  kcYaml = signal('');

  saving = signal(false);
  errorMsg = signal<string | null>(null);

  constructor(private tauri: TauriService) {}

  ngOnInit(): void {
    if (this.editing) {
      const c = this.editing;
      this.name.set(c.name);
      this.environment.set(c.environment);
      this.apiServer.set(c.api_server);
      this.defaultNamespace.set(c.default_namespace);
      this.insecure.set(c.insecure_skip_tls_verify);
      this.authKind.set(c.auth_kind);
      // CA / token / cert never returned from backend; user must re-enter to change.
    }
  }

  async save(): Promise<void> {
    this.errorMsg.set(null);
    this.saving.set(true);
    try {
      if (this.editing && this.mode() === 'kubeconfig') {
        await this.tauri.clusterUpdateFromKubeconfig(this.editing.id, {
          yaml: this.kcYaml(),
          name: this.name() || this.editing.name,
          environment: this.environment(),
        });
      } else if (this.editing && this.mode() === 'manual') {
        await this.tauri.clusterUpdate(this.editing.id, this.buildInput());
      } else if (this.mode() === 'kubeconfig') {
        await this.tauri.clusterImportKubeconfig({
          yaml: this.kcYaml(),
          name: this.name() || 'imported',
          environment: this.environment(),
        });
      } else {
        await this.tauri.clusterAdd(this.buildInput());
      }
      this.saved.emit();
    } catch (e) {
      this.errorMsg.set(String(e));
    } finally {
      this.saving.set(false);
    }
  }

  private buildInput(): ClusterInput {
    let auth: Auth;
    if (this.authKind() === 'bearer_token') {
      auth = { kind: 'bearer_token', token: this.token() };
    } else {
      auth = { kind: 'client_cert', cert_pem: this.certPem(), key_pem: this.keyPem() };
    }
    return {
      name: this.name(),
      environment: this.environment(),
      api_server: this.apiServer(),
      ca_pem: this.caPem(),
      auth,
      default_namespace: this.defaultNamespace(),
      insecure_skip_tls_verify: this.insecure(),
    };
  }

  cancel(): void {
    this.cancelled.emit();
  }
}
