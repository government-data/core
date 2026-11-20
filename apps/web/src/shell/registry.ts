import type { FeatureModule } from "@knime-orchestador/feature-kit";
import { platformFeature } from "@knime-orchestador/feature-platform";
import { qualityFeature } from "@knime-orchestador/feature-quality";
import { settingsAdminFeature, settingsFeature } from "@knime-orchestador/feature-settings";

/**
 * The features mounted into the shell, in registration order.
 *
 * This array is the *only* place a feature is wired into the app. To add a
 * feature: create its package, add it as a dependency of `@knime-orchestador/web`, then
 * import and list it here. No existing feature — and no layout code — changes.
 */
export const features: FeatureModule[] = [
  platformFeature,
  qualityFeature,
  settingsFeature,
  settingsAdminFeature,
];
