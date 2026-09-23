#!/usr/bin/env python3
"""Build the US population table used by the persona sampler.

Input: ACS 1-year PUMS CSV zips (person and housing files) from
https://www2.census.gov/programs-surveys/acs/data/pums/<year>/1-Year/ (csv_pus.zip, csv_hus.zip).
Output (committed, shipped with the app, well under 1 MB):
  crates/core/data/populations/US.csv       weighted cells: age band x gender x region x income x occupation group
  crates/core/data/populations/US_ages.csv  weighted single-year ages (18+), for drawing an age inside a band
  crates/core/data/populations/US.meta.json source, year, record counts

Adults only (18+), people in housing units only (group quarters have no household income).
Weights are person weights (PWGTP). Household income is HINCP adjusted to the survey year with ADJINC.

Usage: python3 build_us.py <dir-with-zips> <year> <output-dir>
Standard library only; streams the files, so memory stays small apart from the household map.
"""
import csv
import io
import json
import sys
import zipfile
from collections import defaultdict
from datetime import date

REGIONS = {"1": "Northeast", "2": "Midwest", "3": "South", "4": "West"}
GENDERS = {"1": "Male", "2": "Female"}


def age_band(age):
    if age < 30:
        return "18–29"
    if age < 45:
        return "30–44"
    if age < 60:
        return "45–59"
    return "60+"


def income_band(income):
    if income < 50_000:
        return "Under $50k"
    if income < 100_000:
        return "$50k–$100k"
    return "Over $100k"


def occupation_group(esr, occp):
    """Major SOC-based groups from the 2018 Census occupation codes (OCCP). Labels contain no commas."""
    if esr == "3":
        return "Unemployed"
    if esr in ("", "6"):
        return "Not in labor force"
    try:
        code = int(occp)
    except ValueError:
        return "Not in labor force"
    if code <= 3550:
        return "Management & professional"
    if code <= 4655:
        return "Service"
    if code <= 5940:
        return "Sales & office"
    if code <= 7640:
        return "Construction & maintenance"
    if code <= 9760:
        return "Production & transport"
    return "Military"


def csv_members(zip_path, prefix):
    with zipfile.ZipFile(zip_path) as z:
        for name in sorted(z.namelist()):
            if name.startswith(prefix) and name.endswith(".csv"):
                with z.open(name) as raw:
                    yield from csv.DictReader(io.TextIOWrapper(raw, encoding="utf-8", newline=""))


def main(src, year, out):
    households = {}
    for row in csv_members(f"{src}/csv_hus.zip", "psam_hus"):
        if row["TYPEHUGQ"] != "1" or row["HINCP"] == "":
            continue
        households[row["SERIALNO"]] = int(row["HINCP"]) * int(row["ADJINC"]) / 1_000_000
    print(f"households with income: {len(households):,}", file=sys.stderr)

    cells = defaultdict(int)
    ages = defaultdict(int)
    used = skipped = 0
    for row in csv_members(f"{src}/csv_pus.zip", "psam_pus"):
        age = int(row["AGEP"])
        income = households.get(row["SERIALNO"])
        if age < 18 or income is None:
            skipped += 1
            continue
        w = int(row["PWGTP"])
        key = (
            age_band(age),
            GENDERS[row["SEX"]],
            REGIONS[row["REGION"]],
            income_band(income),
            occupation_group(row["ESR"], row["OCCP"]),
        )
        cells[key] += w
        ages[age] += w
        used += 1
    print(f"adults used: {used:,}; records skipped: {skipped:,}", file=sys.stderr)

    with open(f"{out}/US.csv", "w", newline="", encoding="utf-8") as f:
        wr = csv.writer(f)
        wr.writerow(["age_band", "gender", "region", "income", "occupation_group", "weight"])
        for key in sorted(cells):
            wr.writerow([*key, cells[key]])
    with open(f"{out}/US_ages.csv", "w", newline="", encoding="utf-8") as f:
        wr = csv.writer(f)
        wr.writerow(["age", "weight"])
        for age in sorted(ages):
            wr.writerow([age, ages[age]])
    meta = {
        "country": "US",
        "source": f"American Community Survey {year} 1-year PUMS (person and housing files), U.S. Census Bureau",
        "url": f"https://www2.census.gov/programs-surveys/acs/data/pums/{year}/1-Year/",
        "universe": "Adults 18+ living in housing units (group quarters excluded)",
        "weight": "PWGTP (person weight); household income HINCP adjusted with ADJINC",
        "records_used": used,
        "weighted_population": sum(cells.values()),
        "cells": len(cells),
        "built": date.today().isoformat(),
        "builder": "tools/build-populations/build_us.py",
    }
    with open(f"{out}/US.meta.json", "w", encoding="utf-8") as f:
        json.dump(meta, f, indent=2)
        f.write("\n")
    print(json.dumps(meta, indent=2), file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3])
