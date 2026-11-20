import {
  DEFAULT_NAV_ORDER,
  isRoleAllowed,
  type FeatureModule,
  type NavItem,
  type SettingsCard,
  type SidebarPanel,
} from "@knime-orchestador/feature-kit";
import type { RouteDefinition } from "@solidjs/router";
import type { Component } from "solid-js";

/**
 * Merge the sidebar entries of every feature into one list, ordered by each
 * entry's `order` weight and, as a tie-breaker, its declaration order. This is
 * the only place navigation is assembled, so a new feature appears in the menu
 * purely by being registered — no shell markup changes.
 */
export function collectNav(features: FeatureModule[], userRole?: string): NavItem[] {
  return features
    .filter((feature) => !userRole || isRoleAllowed(userRole, feature.roles))
    .flatMap((feature) => feature.nav ?? [])
    .filter((item) => !userRole || isRoleAllowed(userRole, item.roles))
    .map((item, index) => ({ item, index }))
    .sort((a, b) => {
      const byOrder = (a.item.order ?? DEFAULT_NAV_ORDER) - (b.item.order ?? DEFAULT_NAV_ORDER);
      return byOrder !== 0 ? byOrder : a.index - b.index;
    })
    .map(({ item }) => item);
}

/**
 * The overlays (UI that stays visible across sections) of the features the
 * user may access, in registration order.
 */
export function collectOverlays(features: FeatureModule[], userRole?: string): Component[] {
  return features
    .filter((feature) => !userRole || isRoleAllowed(userRole, feature.roles))
    .flatMap((feature) => (feature.overlay ? [feature.overlay] : []));
}

/** The sidebar panels of the features the user may access, in registration order. */
export function collectPanels(features: FeatureModule[], userRole?: string): SidebarPanel[] {
  return features
    .filter((feature) => !userRole || isRoleAllowed(userRole, feature.roles))
    .flatMap((feature) => (feature.panel ? [feature.panel] : []));
}

/** The settings cards of the features the user may access. */
export function collectSettingsCards(features: FeatureModule[], userRole?: string): SettingsCard[] {
  return features
    .filter((feature) => !userRole || isRoleAllowed(userRole, feature.roles))
    .flatMap((feature) => feature.settingsCards ?? []);
}

function pathsOf(route: RouteDefinition): string[] {
  const own = Array.isArray(route.path) ? route.path : [route.path];
  const children = route.children
    ? (Array.isArray(route.children) ? route.children : [route.children]).flatMap(pathsOf)
    : [];
  return [...own, ...children];
}

/**
 * Every route path contributed by the features, flattened across nested
 * children. Used for diagnostics and to assert features do not collide.
 */
export function collectRoutePaths(features: FeatureModule[]): string[] {
  return features.flatMap((feature) => feature.routes.flatMap(pathsOf));
}

/**
 * Paths declared by more than one feature route. An empty result means the
 * registered features own disjoint slices of the URL space.
 */
export function duplicateRoutePaths(features: FeatureModule[]): string[] {
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const path of collectRoutePaths(features)) {
    if (seen.has(path)) {
      duplicates.add(path);
    }
    seen.add(path);
  }
  return [...duplicates];
}
