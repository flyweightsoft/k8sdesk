export type Environment = 'dev' | 'staging' | 'prod';

export type AuthKind = 'bearer_token' | 'client_cert';

export interface RedactedCluster {
  id: string;
  name: string;
  environment: Environment;
  api_server: string;
  default_namespace: string;
  auth_kind: AuthKind;
  has_ca: boolean;
  insecure_skip_tls_verify: boolean;
}

export type Auth =
  | { kind: 'bearer_token'; token: string }
  | { kind: 'client_cert'; cert_pem: string; key_pem: string };

export interface ClusterInput {
  name: string;
  environment: Environment;
  api_server: string;
  ca_pem: string;
  auth: Auth;
  default_namespace: string;
  insecure_skip_tls_verify: boolean;
}

export interface KubeconfigImport {
  yaml: string;
  name: string;
  environment: Environment;
}

export type Severity = 'safe' | 'destructive';

export interface ConfirmationRequest {
  challenge_id: string;
  cluster_id: string;
  cluster_name: string;
  environment: Environment;
  action_summary: string;
  require_typed_name: boolean;
}

export type CommandOutput =
  | { kind: 'table'; headers: string[]; rows: string[][] }
  | { kind: 'yaml'; body: string }
  | { kind: 'text'; body: string }
  | { kind: 'ok'; message: string };

export type ExecuteResponse =
  | { kind: 'output'; severity: Severity; output: CommandOutput }
  | { kind: 'needs_confirmation'; request: ConfirmationRequest };

export interface DashboardSession {
  /** Base URL of the local proxy, e.g. http://127.0.0.1:PORT */
  url: string;
  /** Bearer token – shown as a manual fallback if auto-login fails */
  token: string;
}

export interface ExecuteRequest {
  cluster_id: string;
  namespace: string;
  command: string;
  confirmation?: string;
  apply_body?: string;
}
