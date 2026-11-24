/**
 * What the explorer loads at its top: the sources and the datasets, each
 * as nothing when the person may not view them, and the datasets as none
 * when the installation has no catalog, so the sources stay.
 */

import { noCatalog, QualityError, rulesApi, sourcesApi } from "./api";

/** What the person may not view, as nothing: the rest of the tree is still there. */
const orNone = <T>(call: Promise<T[]>): Promise<T[]> =>
  call.catch((e: unknown) =>
    e instanceof QualityError && e.status === 403 ? [] : Promise.reject(e),
  );

export const sourcesOrNone = () => orNone(sourcesApi.list());

/** No datasets when the installation has no catalog, said by `missing`. */
export const datasetsOrNone = (missing: (none: boolean) => void) =>
  orNone(rulesApi.datasets()).then(
    (datasets) => (missing(false), datasets),
    (e: unknown) => (noCatalog(e) ? (missing(true), []) : Promise.reject(e)),
  );
