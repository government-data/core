# /// script
# requires-python = ">=3.11"
# dependencies = ["pandas", "numpy", "pyarrow", "email-validator", "pyyaml"]
# ///
"""The parity report of decision 0033 (step 4): the prototype's rules (its
reglas_vigentes.py, reglas_propuestas.py and patrones.py, a package `calidad`, not in
this repository) run on a local cut built as the KNIME flow builds it, and
their failures per rule compared with what `server quality rules test`
gave for the same cut. Development only: the cut stays in tmp/ and only
counts are printed.

    server quality rules test examples/rules/clientes_naturales/*.yaml ... > ours_pn.txt
    server quality rules test examples/rules/clientes_juridicas/*.yaml ... > ours_pj.txt
    uv run tools/dev/prototype_parity.py --prototype DIR --data tmp/datos \\
        --cut 2021-03-10 ours_pn.txt ours_pj.txt

The flow is read from the mappings in examples/datasets (its Column Rename
nodes), plus what it computes: INF FINANCIERA's totals for legal persons
and Transformación's ages, counted to the cut instead of the day it runs."""
import argparse, sys
from pathlib import Path

import pandas as pd
import yaml

ROOT = Path(__file__).resolve().parents[2]
FIN_INCOME = ["InterDescuentos", "OtrosIntereses", "Valorizaciones", "RendimientoInv", "Comisiones",
              "UtilVentaInversion", "RecupOperacionales", "Diversos", "IngresosServicios"]


def load(dataset, data, client_type):
    mapping = yaml.safe_load(open(ROOT / "examples/datasets" / f"{dataset}.yaml"))
    df = pd.read_parquet(Path(data) / f"{dataset}.parquet")
    df = df.rename(columns={f["column"]: name for name, f in mapping["fields"].items() if f["column"] != name})
    if client_type == "PJ":  # INF FINANCIERA, by kind of entity
        fin, nofin = df["TipoPersonaJuridica"].eq("FINANCIERA"), df["TipoPersonaJuridica"].eq("NO FINANCIERA")
        totals = {"TotalIngresos": (sum(df[c] for c in FIN_INCOME), df["IngVentasNetas"] + df["IngresosFinanc"]),
                  "OtrosIngresos": (df["OtrosIngresosFuente"], df["OtrIngresosNoFin"]),
                  "TotalEgresos": (df["TotalEgresosFuente"], df["TotEgresosNoFin"]),
                  "Activo": (df["ActivosTotales"], df["TotACtivosNoFin"]),
                  "Pasivo": (df["PasivosTotal"], df["TotPasivosNoFin"]),
                  "Patrimonio": (df["Patriminio"], df["TotPatrimoNoFin"])}
        for f, (a, b) in totals.items():
            df[f] = a.where(fin, b.where(nofin))
    df["TipoCliente"] = client_type
    return df


def prototype_counts(prototype, data, cut):
    sys.path.insert(0, prototype)
    from calidad.patrones import Ctx
    from calidad.reglas_propuestas import reglas as propuestas
    from calidad.reglas_vigentes import reglas as vigentes
    df = pd.concat([load("clientes_naturales", data, "PN"), load("clientes_juridicas", data, "PJ")], ignore_index=True, sort=False)
    # The advisors' master, joined as the flow joins it, by the advisor's code.
    advisors = pd.read_parquet(Path(data) / "asesores.parquet")[["CodigoEscalaAsesor", "CedulaAsesor", "EstadoAsesor"]]
    advisors["CodigoAsesor"] = advisors.pop("CodigoEscalaAsesor").astype("Int64").astype(str)
    df["CodigoAsesor"] = df["CodigoAsesor"].astype("string").str.strip()
    df = df.merge(advisors.drop_duplicates("CodigoAsesor"), on="CodigoAsesor", how="left")
    df["CodigoCiiuFuente"] = df["CodigoCiiu"]  # the source's code: the flow repairs it later
    hoy = pd.Timestamp(cut)
    days = lambda c: (hoy - pd.to_datetime(df[c], errors="coerce")).dt.days  # Transformación, to the cut
    df["Edad"] = (days("FechaNacimientoCliente") / 365.25).round(2)
    df["AnnosExpedicionCedula"] = (days("FechaExpedicionDocumento") / 365.25).round(3)
    df["DiasActualizacion"] = days("FechaUltimaActualizacionCliente")
    x = Ctx(df, hoy)
    counts = {}
    for r in vigentes() + propuestas():
        try:
            counts[r.id] = int((r.poblacion(x) & r.falla(x).fillna(False).astype(bool)).sum())
        except KeyError:
            counts[r.id] = None
    return counts


def our_counts(files):
    """Failures per rule; a rule in several datasets (the proposed ones,
    for every client) adds them up, as the prototype counts all clients."""
    counts = {}
    for f in files:
        for line in open(f):
            p = line.split()
            if len(p) >= 6 and p[2].isdigit():
                counts[p[0]] = counts.get(p[0], 0) + int(p[2])
    return counts


def main():
    a = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    a.add_argument("--prototype", required=True, help="the directory holding the package `calidad`")
    a.add_argument("--data", default=str(ROOT / "tmp/datos"))
    a.add_argument("--cut", required=True)
    a.add_argument("ours", nargs="+", help="outputs of `server quality rules test`")
    args = a.parse_args()
    theirs, ours = prototype_counts(args.prototype, args.data, args.cut), our_counts(args.ours)
    rows = [(r, theirs[r], ours.get(r)) for r in theirs]
    equal = [r for r in rows if r[1] is not None and r[1] == r[2]]
    print(f"rules {len(rows)} · equal {len(equal)} · different {sum(1 for r in rows if None not in r[1:] and r[1] != r[2])}"
          f" · not translated {sum(1 for r in rows if r[2] is None)} · not evaluable in the prototype {sum(1 for r in rows if r[1] is None)}")
    for rid, p, o in rows:
        if p is not None and o is not None and p != o:
            print(f"  {rid:9} prototype {p:>8}  rules {o:>8}  ({o - p:+d})")


if __name__ == "__main__":
    main()
