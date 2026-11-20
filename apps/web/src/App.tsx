import { Route, Router } from "@solidjs/router";
import { For, type Component, type ParentProps } from "solid-js";
import { lang } from "@knime-orchestador/i18n";
import { createApiClient } from "@knime-orchestador/api-client";
import { PUBLIC_PATHS, signedOut } from "@knime-orchestador/feature-kit";
import { AppQueryProvider, redirectToLoginOnUnauthorized } from "@knime-orchestador/query/provider";
import { AdminLayout } from "./shell/AdminLayout";
import { FeatureRoutes } from "./shell/routes";
import { ForgotPassword } from "./auth/ForgotPassword";
import { CentralLogin, Handoff, isCentralHost } from "./auth/CentralLogin";
import { Login } from "./auth/Login";
import { ACCOUNT_HOME, isAccount } from "./auth/account";
import { ActivateAccount, ResetPassword } from "./auth/ResetPassword";
import { RequireSession } from "./auth/session";

/**
 * The admin route group: a valid session is required before any of it renders,
 * and the admin chrome wraps whatever feature route matched.
 */
const AdminShell: Component<ParentProps> = (props) => (
  <RequireSession>
    <AdminLayout>{props.children}</AdminLayout>
  </RequireSession>
);

/** In the common account, whatever is not an access screen belongs to the chooser. */
const ToAccountHome: Component = () => {
  window.location.replace(ACCOUNT_HOME);
  return null;
};

/**
 * The single api-client instance every feature's queries fetch through. It is
 * also handed to the 401 interceptor below so an unauthorized response logs the
 * session out server-side before redirecting.
 */
const api = createApiClient();

/**
 * The application shell. It provides the shared TanStack Query client (with the
 * global 401 → logout+redirect interceptor) and defines the two route groups:
 * the chrome-less `/login`, `/handoff`, `/forgot-password` and `/reset-password` screens,
 * and everything else nested under the admin layout. Feature routes are discovered through the shell registry — the shell
 * has no knowledge of any specific feature.
 */
export const App: Component = () => {
  return (
    <AppQueryProvider
      onUnauthorized={redirectToLoginOnUnauthorized({
        logout: () => {
          signedOut();
          return api.POST("/auth/logout");
        },
        publicPaths: PUBLIC_PATHS,
      })}
    >
      {/* In another language the app is built again (no page reload): what
          a screen worked out once from the texts follows too. The cached
          data stays. */}
      <For each={[lang()]}>
        {() => (
          <>
            {/* The common entrance (entrar.<domain>): only the sign-in. */}
            {isCentralHost() ? (
              <Router>
                <Route path="*" component={CentralLogin} />
              </Router>
            ) : isAccount() ? (
              // The common account (cuenta.<domain>): only the access screens.
              <Router>
                <Route path="/login" component={Login} />
                <Route path="/forgot-password" component={ForgotPassword} />
                <Route path="/reset-password" component={ResetPassword} />
                <Route path="/activate" component={ActivateAccount} />
                <Route path="*" component={ToAccountHome} />
              </Router>
            ) : (
              <Router>
                <Route path="/login" component={Login} />
                <Route path="/handoff" component={Handoff} />
                <Route path="/forgot-password" component={ForgotPassword} />
                <Route path="/reset-password" component={ResetPassword} />
                <Route path="/activate" component={ActivateAccount} />
                <Route path="/" component={AdminShell}>
                  {FeatureRoutes()}
                </Route>
              </Router>
            )}
          </>
        )}
      </For>
    </AppQueryProvider>
  );
};
