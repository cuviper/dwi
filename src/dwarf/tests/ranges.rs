use super::*;

pub fn fixture(version: u16, endian: RunTimeEndian) -> Database {
    use write::{Address, AttributeValue as V, Range, RangeList};
    let mut dwarf = write::Dwarf::default();
    let encoding = Encoding {
        format: Format::Dwarf32,
        version,
        address_size: 8,
    };
    let mut unit = write::Unit::new(encoding, write::LineProgram::none());
    let root = unit.root();
    unit.get_mut(root)
        .set(gimli::DW_AT_low_pc, V::Address(Address::Constant(0x1000)));
    let list = unit.ranges.add(RangeList(vec![
        Range::OffsetPair {
            begin: 0x10,
            end: 0x20,
        },
        Range::BaseAddress {
            address: Address::Constant(0x2000),
        },
        Range::OffsetPair {
            begin: 0x20,
            end: 0x30,
        },
    ]));
    let function = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(function)
        .set(gimli::DW_AT_ranges, V::RangeListRef(list));
    let alias = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(alias)
        .set(gimli::DW_AT_specification, V::UnitRef(function));
    let empty_list = unit.ranges.add(RangeList(vec![]));
    let empty = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(empty)
        .set(gimli::DW_AT_ranges, V::RangeListRef(empty_list));
    dwarf.units.add(unit);
    from_written(dwarf, endian)
}

#[test]
fn range_lists_use_cu_and_explicit_bases_in_both_endiannesses() {
    for version in [4, 5] {
        for endian in [RunTimeEndian::Little, RunTimeEndian::Big] {
            let mut db = fixture(version, endian);
            let dies = db.children(db.roots[0]).unwrap();
            let attr = db.attributes(dies[0].id).unwrap().remove(0);
            assert_eq!(attr.name, "ranges");
            assert!(attr.has_list());
            assert!(attr.location_list.is_none());
            assert!(attr.value.starts_with(if version == 4 {
                ".debug_ranges+"
            } else {
                ".debug_rnglists+"
            }));
            let ranges = db.ranges(attr.owner, attr.range_list.unwrap()).unwrap();
            assert_eq!(
                ranges
                    .iter()
                    .map(|range| range.name.as_str())
                    .collect::<Vec<_>>(),
                ["[0x1010, 0x1020)", "[0x2020, 0x2030)"]
            );
            assert!(
                ranges.iter().all(|range| !range.has_list()
                    && !range.is_expression
                    && range.value.is_empty())
            );
            let empty = db.attributes(dies[2].id).unwrap().remove(0);
            assert!(
                db.ranges(empty.owner, empty.range_list.unwrap())
                    .unwrap()
                    .is_empty()
            );
            assert!(
                db.ranges(
                    attr.owner,
                    RangeListRef::Offset(gimli::RawRangeListsOffset(usize::MAX))
                )
                .is_err()
            );
        }
    }
}

#[test]
fn indexed_range_lists_resolve_list_and_address_indices() {
    let abbrev = [
        1, 0x11, 1, 0x74, 0x17, 0x73, 0x17, 0, 0, // CU: rnglists_base, addr_base
        2, 0x2e, 0, 0x55, 0x23, 0, 0, // subprogram: ranges / rnglistx
        0,
    ];
    let info = [
        20, 0, 0, 0, 5, 0, 1, 8, 0, 0, 0, 0, // header
        1, 12, 0, 0, 0, 8, 0, 0, 0, // table bases
        2, 0, // subprogram, range list index 0
        0,
    ];
    let lists = [
        21, 0, 0, 0, 5, 0, 8, 0, 1, 0, 0, 0, // header: one offset
        4, 0, 0, 0, // list at base 12 + 4
        1, 0, // base_addressx 0
        4, 0x10, 0x20, // offset_pair
        3, 1, 0x10, // startx_length index 1
        0,
    ];
    let mut addresses = vec![20, 0, 0, 0, 5, 0, 8, 0];
    addresses.extend_from_slice(&0x4000u64.to_le_bytes());
    addresses.extend_from_slice(&0x5000u64.to_le_bytes());
    let dwarf = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes: &[u8] = match id {
            gimli::SectionId::DebugInfo => &info,
            gimli::SectionId::DebugAbbrev => &abbrev,
            gimli::SectionId::DebugRngLists => &lists,
            gimli::SectionId::DebugAddr => &addresses,
            _ => &[],
        };
        Ok(RelocateReader::new(
            EndianRcSlice::new(Rc::from(bytes), RunTimeEndian::Little),
            Relocations(Rc::new(object::RelocationMap::default())),
        ))
    })
    .unwrap();
    let mut db = Database::from_dwarf(dwarf).unwrap();
    let function = db.children(db.roots[0]).unwrap().remove(0);
    let attr = db.attributes(function.id).unwrap().remove(0);
    assert_eq!(attr.form, "rnglistx");
    assert_eq!(attr.value, ".debug_rnglists index 0");
    let ranges = db.ranges(attr.owner, attr.range_list.unwrap()).unwrap();
    assert_eq!(
        ranges
            .iter()
            .map(|range| range.name.as_str())
            .collect::<Vec<_>>(),
        ["[0x4010, 0x4020)", "[0x5000, 0x5010)"]
    );
}

#[test]
fn supplementary_ranges_are_read_from_the_supplementary_sections() {
    let mut main = super::fixture(5, RunTimeEndian::Little);
    let sup = fixture(5, RunTimeEndian::Little);
    main.dwarf.set_sup(sup.dwarf);
    let mut db = Database::from_dwarf(main.dwarf).unwrap();
    let function = db.children(db.roots[2]).unwrap().remove(0);
    assert_eq!(db.section(function.id), DieSection::SupplementaryInfo);
    let attr = db.attributes(function.id).unwrap().remove(0);
    let ranges = db.ranges(attr.owner, attr.range_list.unwrap()).unwrap();
    assert_eq!(ranges[0].name, "[0x1010, 0x1020)");
}
