/**
 * The cloud's common account (`cuenta.<domain>`, crates/account): there this
 * app only serves the access screens (sign in with its captcha, recover and
 * choose the password). After signing in it goes to the account's chooser
 * (`/home`: management, each company's app, the billing) or to the
 * application that sent it to sign in (`/oauth/authorize`), which are not
 * part of this app: they load whole.
 */
export const ACCOUNT_HOST = "cuenta.";

/** The account's chooser, where signing in leads. */
export const ACCOUNT_HOME = "/home";

/** Whether this page is the common account. */
export function isAccount(hostname = window.location.hostname): boolean {
  return hostname.startsWith(ACCOUNT_HOST);
}

/** Go to another page of the account, whole: only a path of this site. */
export function goInAccount(to: string): void {
  const path = to.split("?", 1)[0];
  const local = to.startsWith("/") && !path.includes("//") && !path.includes("\\");
  window.location.replace(local ? to : ACCOUNT_HOME);
}

/** The product's site (the "home"): the account's domain without `cuenta.`. */
export function accountSite(location: Location = window.location): string {
  return `${location.protocol}//${location.host.replace(/^cuenta\./, "")}`;
}
