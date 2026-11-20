# Rules of the prototype not translated yet

Each needs what is named; none is guessed.

| Rule     | Field             | Needs                                                                                                        |
| -------- | ----------------- | ------------------------------------------------------------------------------------------------------------ |
| P-04     | NombreRazonSocial | NombreRazonSocial is not a field of the natural persons' flow, and the rule is for natural persons only      |
| P-16     | CodigoCiudad      | it compares department names without accents, which the language cannot do yet                               |
| V-PJ-06  | DireccionLaboral  | the flow has no DireccionLaboral for legal persons: it alerts all of them; left out (decided 2026-10-30)     |
| V-PJ-07  | DireccionLaboral  | the flow has no DireccionLaboral for legal persons: it alerts all of them; left out (decided 2026-10-30)     |
| V-PJ-08  | DireccionLaboral  | the flow has no DireccionLaboral for legal persons: it alerts all of them; left out (decided 2026-10-30)     |
| V-PJ-10  | Transacciones     | data not in this extract (transactions)                                                                      |
| V-PJ-11  | Estrato           | the flow has no Estrato for legal persons: it alerts all of them; left out (decided 2026-10-30)              |
| V-PJ-25  | NroTrabajadores   | the flow does not rename NumTrabaja to NroTrabajadores: it alerts all of them; left out (decided 2026-10-30) |
| V-PN-101 | ParentescoRef1    | the flow's condition contradicts itself and its intent is not clear                                          |
| V-PN-107 | ParentescoRef2    | the flow's condition contradicts itself and its intent is not clear                                          |
| V-PN-115 | Activo            | data not in this extract (savings, contributions)                                                            |
| V-PN-116 | Pasivo            | data not in this extract (loans)                                                                             |
| V-PN-50  | Correspondencia   | OtraDireccion is not a field of the flow                                                                     |
| V-PN-60  | Transacciones     | data not in this extract (transactions)                                                                      |
| V-PN-90  | NombreRazonSocial | NombreRazonSocial is not a field of the natural persons' flow                                                |
| V-PN-91  | NombreRazonSocial | NombreRazonSocial is not a field of the natural persons' flow                                                |
