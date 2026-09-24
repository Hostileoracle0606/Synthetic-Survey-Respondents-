#!/usr/bin/env python3
"""Build the Canada population table used by the persona sampler.

Input: the 2021 Census Individuals PUMF from Statistics Canada (catalogue 98M0001X), file
cen21_ind_98m0001x_part_rec21.zip from https://www150.statcan.gc.ca/n1/pub/98m0001x/index-eng.htm,
unzipped. Released under the Statistics Canada Open Licence.
Output (committed, shipped with the app, well under 1 MB):
  crates/core/data/populations/CA.csv       weighted cells: age band x gender x region x income x occupation group
  crates/core/data/populations/CA_ages.csv  weighted single-year ages (18+), for drawing an age inside a band
  crates/core/data/populations/CA.meta.json source, year, record counts

Adults only (18+). The PUMF covers people in private households. Records whose household income,
labour force status or occupation is "not available" are skipped (about 1.7% of adults). Weights are the individuals weighting factor (WEIGHT). Household income
(HHInc) is 2020 income in Canadian dollars. The PUMF gives age in groups (AGEGRP), not single
years, so each group's weight is spread evenly over its years; 85+ is spread over 85-89.

Usage: python3 build_ca.py <path-to-data_donnees_2021_ind_v2.csv> <output-dir>
Standard library only; streams the file.
"""
import csv
import json
import sys
from collections import defaultdict
from datetime import date

# AGEGRP code -> (first year, last year). Codes below 7 are under 18; 88 is not available.
AGE_GROUPS = {
    7: (18, 19), 8: (20, 24), 9: (25, 29), 10: (30, 34), 11: (35, 39), 12: (40, 44),
    13: (45, 49), 14: (50, 54), 15: (55, 59), 16: (60, 64), 17: (65, 69), 18: (70, 74),
    19: (75, 79), 20: (80, 84), 21: (85, 89),
}
GENDERS = {"1": "Female", "2": "Male"}  # PUMF Gender: 1 Woman+, 2 Man+
REGIONS = {
    "10": "Atlantic", "11": "Atlantic", "12": "Atlantic", "13": "Atlantic",
    "24": "Quebec", "35": "Ontario",
    "46": "Prairies", "47": "Prairies", "48": "Prairies",
    "59": "British Columbia", "70": "Territories",
}
# NOC 2021 major group codes (PUMF variable NOC21) -> occupation group. Labels contain no commas.
OCCUPATIONS = {
    **dict.fromkeys([1, 2, 3, 7, 8, 9, 10, 12, 15], "Management & professional"),
    **dict.fromkeys([4, 5, 6, 17], "Sales & office"),
    **dict.fromkeys([11, 13, 14, 16, 18, 19, 20], "Service"),
    **dict.fromkeys([21, 22, 25], "Trades & natural resources"),
    **dict.fromkeys([23, 24, 26], "Production & transport"),
}


def age_band(first_year):
    if first_year < 30:
        return "18–29"
    if first_year < 45:
        return "30–44"
    if first_year < 60:
        return "45–59"
    return "60+"


def income_band(code):
    """HHInc codes 1-14 are under $50,000; 15-24 are $50,000-$99,999; 25-33 are $100,000+."""
    if code <= 14:
        return "Under $50k"
    if code <= 24:
        return "$50k–$100k"
    return "Over $100k"


def occupation_group(lfact, noc):
    """Employed people get their NOC group; everyone else gets their labour force status."""
    if 3 <= lfact <= 10:
        return "Unemployed"
    if lfact in (1, 2):
        return OCCUPATIONS.get(noc)
    return "Not in labour force"


def main(src, out):
    cells = defaultdict(float)
    ages = defaultdict(float)
    used = skipped = 0
    with open(src, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            group = AGE_GROUPS.get(int(row["AGEGRP"]))
            income = int(row["HHInc"])
            occupation = occupation_group(int(row["LFACT"]), int(row["NOC21"]))
            if group is None or income == 88 or occupation is None or row["Gender"] not in GENDERS:
                skipped += 1
                continue
            w = float(row["WEIGHT"])
            key = (
                age_band(group[0]),
                GENDERS[row["Gender"]],
                REGIONS[row["PR"]],
                income_band(income),
                occupation,
            )
            cells[key] += w
            years = range(group[0], group[1] + 1)
            for a in years:
                ages[a] += w / len(years)
            used += 1
    print(f"adults used: {used:,}; records skipped: {skipped:,}", file=sys.stderr)

    with open(f"{out}/CA.csv", "w", newline="", encoding="utf-8") as f:
        wr = csv.writer(f)
        wr.writerow(["age_band", "gender", "region", "income", "occupation_group", "weight"])
        for key in sorted(cells):
            wr.writerow([*key, round(cells[key])])
    with open(f"{out}/CA_ages.csv", "w", newline="", encoding="utf-8") as f:
        wr = csv.writer(f)
        wr.writerow(["age", "weight"])
        for age in sorted(ages):
            wr.writerow([age, round(ages[age])])
    meta = {
        "country": "CA",
        "source": "2021 Census of Population, Individuals Public Use Microdata File (98M0001X), Statistics Canada",
        "url": "https://www150.statcan.gc.ca/n1/pub/98m0001x/index-eng.htm",
        "licence": "Statistics Canada Open Licence",
        "universe": "Adults 18+ in private households; records with income, labour force status or occupation not available are skipped (about 1.7% of adults)",
        "weight": "WEIGHT (individuals weighting factor); household income HHInc is 2020 income in CAD",
        "ages": "Single-year ages spread evenly within each PUMF age group (AGEGRP); 85+ spread over 85-89",
        "records_used": used,
        "weighted_population": round(sum(cells.values())),
        "cells": len(cells),
        "built": date.today().isoformat(),
        "builder": "tools/build-populations/build_ca.py",
    }
    with open(f"{out}/CA.meta.json", "w", encoding="utf-8") as f:
        json.dump(meta, f, indent=2)
        f.write("\n")
    print(json.dumps(meta, indent=2), file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
