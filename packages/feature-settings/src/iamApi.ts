/**
 * Users and access (IAM): the catalog of actions, policies, groups and
 * service accounts.
 */
import { request } from "./http";

/** An action of the catalog and the kind of resource it acts on (`null`:
 * the installation as a whole). */
export interface IamAction {
  name: string;
  resource: string | null;
}

export interface IamCatalog {
  actions: IamAction[];
  resource_kinds: string[];
}

export interface IamStatement {
  effect: "Allow" | "Deny";
  actions: string[];
  resources: string[];
}

export interface IamPolicy {
  name: string;
  description: string;
  statements: IamStatement[];
  /** Shipped with the application: attached, never changed. */
  managed: boolean;
  /** How many accounts, groups and service accounts have it. */
  attached: number;
}

export interface IamGroupMember {
  admin_id: string;
  email: string;
  display_name: string | null;
  /** `manual` (added here) or `directory` (by a directory group). */
  source: "manual" | "directory";
}

export interface IamGroup {
  id: string;
  name: string;
  description: string;
  directory_groups: string[];
  members: IamGroupMember[];
  policies: string[];
}

export interface IamGroupChange {
  name: string;
  description: string;
  directory_groups: string[];
  /** The people added by hand (account ids). */
  members: string[];
  policies: string[];
}

export interface UserAccess {
  admin_id: string;
  policies: string[];
  groups: string[];
}

export interface ServiceAccount {
  id: string;
  owner_id: string;
  owner_email: string;
  name: string;
  access_key: string;
  policies: string[];
  created_at: string;
  last_used_at: string | null;
  expires_at: string | null;
}

/** IAM (ADR 0016): policies, groups, what each account has, service accounts. */
export const iamApi = {
  catalog: () => request<IamCatalog>("GET", "/iam/catalog"),
  policies: () => request<IamPolicy[]>("GET", "/iam/policies"),
  putPolicy: (name: string, p: { description: string; statements: IamStatement[] }) =>
    request<void>("PUT", `/iam/policies/${encodeURIComponent(name)}`, p),
  deletePolicy: (name: string) =>
    request<void>("DELETE", `/iam/policies/${encodeURIComponent(name)}`),
  groups: () => request<IamGroup[]>("GET", "/iam/groups"),
  createGroup: (g: IamGroupChange) => request<{ id: string }>("POST", "/iam/groups", g),
  updateGroup: (id: string, g: IamGroupChange) =>
    request<void>("PUT", `/iam/groups/${encodeURIComponent(id)}`, g),
  deleteGroup: (id: string) => request<void>("DELETE", `/iam/groups/${encodeURIComponent(id)}`),
  users: () => request<UserAccess[]>("GET", "/iam/users"),
  setUserPolicies: (id: string, policies: string[]) =>
    request<void>("PUT", `/iam/users/${encodeURIComponent(id)}/policies`, { policies }),
  serviceAccounts: () => request<ServiceAccount[]>("GET", "/iam/service-accounts"),
  createServiceAccount: (sa: { name: string; policies: string[]; expires_at: string | null }) =>
    request<{ id: string; access_key: string; token: string }>("POST", "/iam/service-accounts", sa),
  deleteServiceAccount: (id: string) =>
    request<void>("DELETE", `/iam/service-accounts/${encodeURIComponent(id)}`),
};
