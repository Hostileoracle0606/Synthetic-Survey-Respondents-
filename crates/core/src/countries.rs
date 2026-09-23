//! The Target Country list. Bundled with the app; only countries with a census table get
//! real joint demographic distributions (docs/DATA_FLOW.md, Step 1).

use std::sync::LazyLock;

use crate::model::CountryOption;

static COUNTRIES: LazyLock<Vec<CountryOption>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/countries.json"))
        .expect("bundled countries.json is valid")
});

pub fn list() -> &'static [CountryOption] {
    &COUNTRIES
}

pub fn find(code: &str) -> Option<&'static CountryOption> {
    COUNTRIES.iter().find(|c| c.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_sorted_by_name_and_codes_are_unique() {
        let names: Vec<_> = list().iter().map(|c| c.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        let mut codes: Vec<_> = list().iter().map(|c| c.code.as_str()).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), list().len());
    }

    #[test]
    fn census_countries_have_regions() {
        for c in list().iter().filter(|c| c.has_census_table) {
            assert!(
                !c.regions.is_empty(),
                "{} has a census table but no regions",
                c.code
            );
        }
        assert!(find("US").unwrap().has_census_table);
    }
}
