/** The product's domain: the installations we host live on its subdomains. */
const PRODUCT_DOMAIN = "orquestador.app";

/**
 * Where to go on sign-out: a hosted installation (`<company>.orquestador.app`)
 * goes back to the product's page; a customer's own, to its sign-in.
 */
export function afterSignOut(hostname: string): string {
  return hostname.endsWith(`.${PRODUCT_DOMAIN}`) ? `https://${PRODUCT_DOMAIN}/` : "/login";
}
