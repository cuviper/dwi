use super::*;
use gimli::{Encoding, Format, write};

pub(crate) mod ranges;

fn expression_text(bytes: &[u8]) -> String {
    expression_text_with_encoding(bytes, RunTimeEndian::Little, Format::Dwarf32, 8)
}

fn expression_text_with_encoding(
    bytes: &[u8],
    endian: RunTimeEndian,
    format: Format,
    address_size: u8,
) -> String {
    let reader = RelocateReader::new(
        EndianRcSlice::new(Rc::from(bytes), endian),
        Relocations(Rc::new(object::RelocationMap::default())),
    );
    format_expression(
        gimli::Expression(reader),
        Encoding {
            format,
            version: 5,
            address_size,
        },
    )
    .unwrap()
}

#[test]
fn expressions_display_encoded_opcodes_without_treating_operands_as_instructions() {
    for (bytes, expected) in [
        (&[0x57][..], "DW_OP_reg7"),
        (&[0x90, 7][..], "DW_OP_regx 7"),
        (&[0x91, 0x70][..], "DW_OP_fbreg -16"),
        (&[0x91, 0x80, 0x7f][..], "DW_OP_fbreg -128"),
        (&[0x91, 0xc0, 0][..], "DW_OP_fbreg 64"),
        (&[0x77, 0x78][..], "DW_OP_breg7 -8"),
        (&[0x92, 7, 0x78][..], "DW_OP_bregx 7 -8"),
        (&[0x37][..], "DW_OP_lit7"),
        (
            &[0x12, 0x14, 0x15, 2][..],
            "DW_OP_dup; DW_OP_over; DW_OP_pick 2",
        ),
        (
            &[0x06, 0x18, 0x94, 4][..],
            "DW_OP_deref; DW_OP_xderef; DW_OP_deref_size 4",
        ),
        (&[0x93, 4][..], "DW_OP_piece 4 bytes"),
        (&[0x9d, 5, 3][..], "DW_OP_bit_piece 5 bits, offset 3 bits"),
        (
            &[0x10, 0x57, 0x9f][..],
            "DW_OP_constu 87; DW_OP_stack_value",
        ),
        (
            &[0x11, 0x7f, 0x23, 0x80, 1][..],
            "DW_OP_consts -1; DW_OP_plus_uconst 128",
        ),
        (&[][..], ""),
    ] {
        assert_eq!(expression_text(bytes), expected);
    }
}

#[test]
fn expression_offsets_and_addresses_respect_encoding() {
    for (endian, operand) in [
        (RunTimeEndian::Little, [0x34, 0x12]),
        (RunTimeEndian::Big, [0x12, 0x34]),
    ] {
        let bytes = [gimli::DW_OP_call2.0, operand[0], operand[1]];
        assert_eq!(
            expression_text_with_encoding(&bytes, endian, Format::Dwarf32, 8),
            "DW_OP_call2 cu+0x1234"
        );
        let bytes = [gimli::DW_OP_skip.0, 0xff, 0xff];
        assert_eq!(
            expression_text_with_encoding(&bytes, endian, Format::Dwarf32, 8),
            "DW_OP_skip -1"
        );
    }
    assert_eq!(
        expression_text_with_encoding(
            &[0x03, 0x78, 0x56, 0x34, 0x12],
            RunTimeEndian::Little,
            Format::Dwarf32,
            4
        ),
        "DW_OP_addr 0x12345678"
    );
    assert_eq!(
        expression_text_with_encoding(
            &[0x9a, 1, 0, 0, 0, 1, 0, 0, 0],
            RunTimeEndian::Little,
            Format::Dwarf64,
            8
        ),
        "DW_OP_call_ref info:0x100000001"
    );
}

#[test]
fn expressions_format_typed_values_references_and_nested_expressions() {
    for (bytes, expected) in [
        (&[0xa5, 7, 0x20][..], "DW_OP_regval_type 7 type=cu+0x20"),
        (&[0xa6, 4, 0x20][..], "DW_OP_deref_type 4 type=cu+0x20"),
        (
            &[0xa4, 0x20, 2, 0x70, 0xff][..],
            "DW_OP_const_type type=cu+0x20 [70 ff] (2 bytes)",
        ),
        (
            &[0x9e, 2, 0x70, 0xff][..],
            "DW_OP_implicit_value [70 ff] (2 bytes)",
        ),
        (
            &[0xa0, 0x20, 0, 0, 0, 0x70][..],
            "DW_OP_implicit_pointer info:0x20 -16",
        ),
        (&[0xa1, 2, 0xa2, 3][..], "DW_OP_addrx 2; DW_OP_constx 3"),
        (
            &[0xa3, 1, 0x57, 0x9f][..],
            "DW_OP_entry_value (DW_OP_reg7); DW_OP_stack_value",
        ),
        (
            &[0xf3, 2, 0x91, 0x70][..],
            "DW_OP_GNU_entry_value (DW_OP_fbreg -16)",
        ),
        (&[0xed, 0, 3][..], "DW_OP_WASM_location local 3"),
    ] {
        assert_eq!(expression_text(bytes), expected);
    }
}

#[test]
fn expression_errors_and_truncation_are_explicit() {
    let text = expression_text(&[0x90]);
    assert!(text.starts_with("DW_OP_regx <decode error:"));
    assert!(text.ends_with("raw [90] (1 byte)"));
    assert!(!expression_text(&[0x96; 32]).ends_with('…'));
    assert!(expression_text(&[0x96; 33]).ends_with("; …"));
    // Nested expressions share the limit instead of multiplying output by 32
    // at each level. This also bounds recursion for deeply nested entry_value.
    let mut nested = vec![0x57];
    for _ in 0..40 {
        let mut outer = vec![gimli::DW_OP_entry_value.0, nested.len() as u8];
        outer.extend(nested);
        nested = outer;
    }
    let text = expression_text(&nested);
    assert_eq!(text.matches("DW_OP_entry_value").count(), 32);
    assert!(text.contains('…'));
    assert!(!text.contains("DW_OP_reg7"));
}

pub fn fixture(version: u16, endian: RunTimeEndian) -> Database {
    use write::AttributeValue as V;
    let encoding = Encoding {
        format: Format::Dwarf32,
        version,
        address_size: 8,
    };
    let mut dwarf = write::Dwarf::default();
    let mut first = write::Unit::new(encoding, write::LineProgram::none());
    let root = first.root();
    first
        .get_mut(root)
        .set(gimli::DW_AT_name, V::String(b"first.c".to_vec()));
    let variable = first.add(root, gimli::DW_TAG_variable);
    first
        .get_mut(variable)
        .set(gimli::DW_AT_name, V::String(b"head".to_vec()));
    first
        .get_mut(variable)
        .set(gimli::DW_AT_const_value, V::Sdata(-7));
    let first_id = dwarf.units.add(first);

    let mut second = write::Unit::new(encoding, write::LineProgram::none());
    let root = second.root();
    second
        .get_mut(root)
        .set(gimli::DW_AT_name, V::String(b"second.c".to_vec()));
    let namespace = second.add(root, gimli::DW_TAG_namespace);
    second
        .get_mut(namespace)
        .set(gimli::DW_AT_name, V::String(b"example".to_vec()));
    let node = second.add(namespace, gimli::DW_TAG_structure_type);
    second
        .get_mut(node)
        .set(gimli::DW_AT_name, V::String(b"Node".to_vec()));
    let pointer = second.add(namespace, gimli::DW_TAG_pointer_type);
    second
        .get_mut(pointer)
        .set(gimli::DW_AT_type, V::UnitRef(node));
    // A reference cycle through containing_type is valid to browse, regardless
    // of whether a particular producer emits this exact combination of tags.
    second
        .get_mut(node)
        .set(gimli::DW_AT_containing_type, V::UnitRef(pointer));
    let second_id = dwarf.units.add(second);
    dwarf.units.get_mut(first_id).get_mut(variable).set(
        gimli::DW_AT_type,
        V::DebugInfoRef(write::DebugInfoRef::Entry(second_id, node)),
    );
    from_written(dwarf, endian)
}

pub fn location_fixture(version: u16, endian: RunTimeEndian) -> Database {
    use write::{Address, AttributeValue as V, Expression, Location, LocationList};
    let mut dwarf = write::Dwarf::default();
    let encoding = Encoding {
        format: Format::Dwarf32,
        version,
        address_size: 8,
    };
    let mut unit = write::Unit::new(encoding, write::LineProgram::none());
    let mut register = Expression::new();
    register.op_reg(gimli::Register(7));
    let mut stack = Expression::new();
    stack.op_fbreg(-16);
    let mut entries = vec![
        Location::BaseAddress {
            address: Address::Constant(0x1000),
        },
        Location::OffsetPair {
            begin: 0x10,
            end: 0x20,
            data: register.clone(),
        },
        Location::BaseAddress {
            address: Address::Constant(0x2000),
        },
        Location::OffsetPair {
            begin: 0x20,
            end: 0x30,
            data: stack,
        },
        Location::OffsetPair {
            begin: 0x30,
            end: 0x40,
            data: Expression::new(),
        },
    ];
    if version >= 5 {
        entries.push(Location::DefaultLocation { data: register });
    }
    let list = unit.locations.add(LocationList(entries));
    let root = unit.root();
    let variable = unit.add(root, gimli::DW_TAG_variable);
    unit.get_mut(variable)
        .set(gimli::DW_AT_location, V::LocationListRef(list));
    let alias = unit.add(root, gimli::DW_TAG_variable);
    unit.get_mut(alias)
        .set(gimli::DW_AT_specification, V::UnitRef(variable));
    let empty_list = unit.locations.add(LocationList(vec![]));
    let empty = unit.add(root, gimli::DW_TAG_variable);
    unit.get_mut(empty)
        .set(gimli::DW_AT_location, V::LocationListRef(empty_list));
    dwarf.units.add(unit);
    from_written(dwarf, endian)
}

#[test]
fn location_lists_resolve_bases_defaults_and_unavailable_values() {
    for version in [4, 5] {
        for endian in [RunTimeEndian::Little, RunTimeEndian::Big] {
            let mut db = location_fixture(version, endian);
            let dies = db.children(db.roots[0]).unwrap();
            let attr = db.attributes(dies[0].id).unwrap().remove(0);
            assert!(attr.value.starts_with(if version == 4 {
                ".debug_loc+"
            } else {
                ".debug_loclists+"
            }));
            assert!(!attr.is_expression);
            let entries = db
                .locations(attr.owner, attr.location_list.unwrap())
                .unwrap();
            assert_eq!(entries[0].name, "[0x1010, 0x1020)");
            assert_eq!(entries[0].value, "DW_OP_reg7");
            assert_eq!(entries[1].name, "[0x2020, 0x2030)");
            assert_eq!(entries[1].value, "DW_OP_fbreg -16");
            assert_eq!(entries[1].raw, "[91 70] (2 bytes)");
            assert!(entries[1].is_expression);
            assert_eq!(entries[2].name, "[0x2030, 0x2040)");
            assert_eq!(entries[2].value, "<unavailable>");
            assert_eq!(entries.len(), if version == 4 { 3 } else { 4 });
            if version == 5 {
                assert_eq!(entries[3].name, "default");
            }
            let empty = db.attributes(dies[2].id).unwrap().remove(0);
            assert!(
                db.locations(empty.owner, empty.location_list.unwrap())
                    .unwrap()
                    .is_empty()
            );
            assert!(
                db.locations(
                    attr.owner,
                    LocationListRef::Offset(gimli::LocationListsOffset(usize::MAX))
                )
                .is_err()
            );
        }
    }
}

#[test]
fn indexed_location_lists_resolve_list_and_address_tables() {
    // A loclistx attribute uses the CU's table bases for both its list offset
    // and its base_addressx/startx_length entries.
    let abbrev = [
        1, 0x11, 1, 0x8c, 1, 0x17, 0x73, 0x17, 0, 0, // CU: loclists_base, addr_base
        2, 0x34, 0, 2, 0x22, 0, 0, // variable: location / loclistx
        0,
    ];
    let info = [
        20, 0, 0, 0, 5, 0, 1, 8, 0, 0, 0, 0, // DWARF5 CU header
        1, 12, 0, 0, 0, 8, 0, 0, 0, // table bases
        2, 0, // variable, location list index 0
        0,
    ];
    let mut lists = vec![
        0, 0, 0, 0, 5, 0, 8, 0, 1, 0, 0, 0, // header: one offset
        4, 0, 0, 0, // list at base 12 + 4
        1, 0, // base_addressx 0
        4, 0x10, 0x20, 1, 0x57, // offset_pair, reg7
        3, 1, 0x10, 2, 0x91, 0x70, // startx_length index 1, fbreg -16
        0,
    ];
    let length = (lists.len() - 4) as u32;
    lists[..4].copy_from_slice(&length.to_le_bytes());
    let mut addresses = vec![20, 0, 0, 0, 5, 0, 8, 0];
    addresses.extend_from_slice(&0x4000u64.to_le_bytes());
    addresses.extend_from_slice(&0x5000u64.to_le_bytes());
    let dwarf = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes: &[u8] = match id {
            gimli::SectionId::DebugInfo => &info,
            gimli::SectionId::DebugAbbrev => &abbrev,
            gimli::SectionId::DebugLocLists => &lists,
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
    let variable = db.children(db.roots[0]).unwrap().remove(0);
    let attr = db.attributes(variable.id).unwrap().remove(0);
    assert_eq!(attr.form, "loclistx");
    assert_eq!(attr.value, ".debug_loclists index 0");
    let entries = db
        .locations(attr.owner, attr.location_list.unwrap())
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "[0x4010, 0x4020)");
    assert_eq!(entries[1].name, "[0x5000, 0x5010)");
    assert_eq!(entries[1].value, "DW_OP_fbreg -16");
}

fn from_written(mut dwarf: write::Dwarf, endian: RunTimeEndian) -> Database {
    let mut sections = write::Sections::new(write::EndianVec::new(endian));
    dwarf.write(&mut sections).unwrap();
    let reader = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes = sections
            .get(id)
            .map(|section| section.slice())
            .unwrap_or(&[]);
        Ok(RelocateReader::new(
            EndianRcSlice::new(Rc::from(bytes), endian),
            Relocations(Rc::new(object::RelocationMap::default())),
        ))
    })
    .unwrap();
    Database::from_dwarf(reader).unwrap()
}

#[test]
fn inherited_names_follow_chains_and_prefer_source_names() {
    use write::AttributeValue as V;
    let encoding = Encoding {
        format: Format::Dwarf32,
        version: 5,
        address_size: 8,
    };
    let mut dwarf = write::Dwarf::default();
    let mut declarations = write::Unit::new(encoding, write::LineProgram::none());
    let root = declarations.root();
    let declaration = declarations.add(root, gimli::DW_TAG_subprogram);
    declarations
        .get_mut(declaration)
        .set(gimli::DW_AT_name, V::String(b"method".to_vec()));
    let middle = declarations.add(root, gimli::DW_TAG_subprogram);
    declarations
        .get_mut(middle)
        .set(gimli::DW_AT_specification, V::UnitRef(declaration));
    let linkage_only = declarations.add(root, gimli::DW_TAG_variable);
    declarations.get_mut(linkage_only).set(
        gimli::DW_AT_linkage_name,
        V::String(b"_Z8variable".to_vec()),
    );
    let declarations_id = dwarf.units.add(declarations);

    let mut definitions = write::Unit::new(encoding, write::LineProgram::none());
    let root = definitions.root();
    let definition = definitions.add(root, gimli::DW_TAG_subprogram);
    definitions.get_mut(definition).set(
        gimli::DW_AT_specification,
        V::DebugInfoRef(write::DebugInfoRef::Entry(declarations_id, middle)),
    );
    definitions
        .get_mut(definition)
        .set(gimli::DW_AT_linkage_name, V::String(b"_Z6method".to_vec()));
    let inline = definitions.add(root, gimli::DW_TAG_inlined_subroutine);
    definitions
        .get_mut(inline)
        .set(gimli::DW_AT_abstract_origin, V::UnitRef(definition));
    let named = definitions.add(root, gimli::DW_TAG_subprogram);
    definitions
        .get_mut(named)
        .set(gimli::DW_AT_name, V::String(b"local".to_vec()));
    definitions
        .get_mut(named)
        .set(gimli::DW_AT_specification, V::UnitRef(definition));
    let both = definitions.add(root, gimli::DW_TAG_subprogram);
    definitions
        .get_mut(both)
        .set(gimli::DW_AT_abstract_origin, V::UnitRef(definition));
    definitions
        .get_mut(both)
        .set(gimli::DW_AT_specification, V::UnitRef(named));
    let variable = definitions.add(root, gimli::DW_TAG_variable);
    definitions.get_mut(variable).set(
        gimli::DW_AT_abstract_origin,
        V::DebugInfoRef(write::DebugInfoRef::Entry(declarations_id, linkage_only)),
    );
    let local_linkage = definitions.add(root, gimli::DW_TAG_variable);
    definitions
        .get_mut(local_linkage)
        .set(gimli::DW_AT_abstract_origin, V::UnitRef(variable));
    definitions.get_mut(local_linkage).set(
        gimli::DW_AT_linkage_name,
        V::String(b"local_linkage".to_vec()),
    );
    dwarf.units.add(definitions);

    let mut db = from_written(dwarf, RunTimeEndian::Little);
    let dies = db.children(db.roots[1]).unwrap();
    assert_eq!(
        dies.iter().map(|die| die.name.as_str()).collect::<Vec<_>>(),
        [
            "method",
            "method",
            "local",
            "method",
            "_Z8variable",
            "local_linkage"
        ]
    );
    // Direct reads and reference labels use the same inherited name lookup.
    assert_eq!(db.die(dies[0].id).unwrap().name, "method");
    let attrs = db.attributes(dies[1].id).unwrap();
    let origin = attrs
        .iter()
        .find(|attr| attr.name == "abstract_origin")
        .unwrap();
    assert!(origin.value.ends_with("subprogram method"));
    assert_eq!(origin.target, Some(dies[0].id));
    assert!(!attrs.iter().any(|attr| attr.name == "name"));
}

#[test]
fn name_cycles_and_unavailable_origins_do_not_prevent_browsing() {
    use write::AttributeValue as V;
    let encoding = Encoding {
        format: Format::Dwarf32,
        version: 5,
        address_size: 8,
    };
    let mut dwarf = write::Dwarf::default();
    let mut unit = write::Unit::new(encoding, write::LineProgram::none());
    let root = unit.root();
    let a = unit.add(root, gimli::DW_TAG_subprogram);
    let b = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(a)
        .set(gimli::DW_AT_specification, V::UnitRef(b));
    unit.get_mut(b)
        .set(gimli::DW_AT_abstract_origin, V::UnitRef(a));
    let fallback = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(fallback)
        .set(gimli::DW_AT_specification, V::UnitRef(a));
    unit.get_mut(fallback)
        .set(gimli::DW_AT_linkage_name, V::String(b"fallback".to_vec()));
    let missing = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(missing).set(
        gimli::DW_AT_abstract_origin,
        V::DebugInfoRefSup(gimli::DebugInfoOffset(0)),
    );
    let missing_fallback = unit.add(root, gimli::DW_TAG_subprogram);
    unit.get_mut(missing_fallback)
        .set(gimli::DW_AT_specification, V::UnitRef(missing));
    unit.get_mut(missing_fallback).set(
        gimli::DW_AT_MIPS_linkage_name,
        V::String(b"legacy".to_vec()),
    );
    unit.add(root, gimli::DW_TAG_subprogram);
    dwarf.units.add(unit);
    let mut db = from_written(dwarf, RunTimeEndian::Little);
    let dies = db.children(db.roots[0]).unwrap();
    assert!(dies[0].name.contains("cycle"));
    assert!(dies[1].name.contains("cycle"));
    assert_eq!(dies[2].name, "fallback");
    assert!(dies[3].name.contains("supplementary"));
    assert_eq!(dies[4].name, "legacy");
    assert_eq!(dies[5].name, "");
    assert!(!db.attributes(dies[0].id).unwrap().is_empty());
}

#[test]
fn inherited_indexed_strings_use_the_attribute_owners_unit() {
    // Both CUs use strx1 index 0, but have different str_offsets_base values.
    // Resolving a name using the definition's unit would incorrectly give "wrong".
    let abbrev = [
        1, 0x11, 1, 0x72, 0x17, 0, 0, // CU: str_offsets_base / sec_offset
        2, 0x2e, 0, 3, 0x25, 0, 0, // subprogram: name / strx1
        3, 0x2e, 0, 0x47, 0x10, 0, 0, // subprogram: specification / ref_addr
        0,
    ];
    let mut info = Vec::new();
    for (base, body) in [(8u32, vec![3, 40, 0, 0, 0]), (20, vec![2, 0])] {
        let start = info.len();
        info.extend_from_slice(&0u32.to_le_bytes());
        info.extend_from_slice(&[5, 0, 1, 8, 0, 0, 0, 0]);
        info.push(1);
        info.extend_from_slice(&base.to_le_bytes());
        info.extend_from_slice(&body);
        info.push(0);
        let length = (info.len() - start - 4) as u32;
        info[start..start + 4].copy_from_slice(&length.to_le_bytes());
    }
    let mut offsets = Vec::new();
    for offset in [0u32, 6] {
        offsets.extend_from_slice(&[8, 0, 0, 0, 5, 0, 0, 0]);
        offsets.extend_from_slice(&offset.to_le_bytes());
    }
    let dwarf = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes: &[u8] = match id {
            gimli::SectionId::DebugInfo => &info,
            gimli::SectionId::DebugAbbrev => &abbrev,
            gimli::SectionId::DebugStr => b"wrong\0target\0",
            gimli::SectionId::DebugStrOffsets => &offsets,
            _ => &[],
        };
        Ok(RelocateReader::new(
            EndianRcSlice::new(Rc::from(bytes), RunTimeEndian::Little),
            Relocations(Rc::new(object::RelocationMap::default())),
        ))
    })
    .unwrap();
    let mut db = Database::from_dwarf(dwarf).unwrap();
    let definition = db.children(db.roots[0]).unwrap().remove(0);
    assert_eq!(definition.name, "target");
    assert!(
        db.attributes(definition.id).unwrap()[0]
            .value
            .ends_with("subprogram target")
    );
}

#[test]
fn lazy_children_cross_unit_references_and_raw_forms() {
    for version in [4, 5] {
        for endian in [RunTimeEndian::Little, RunTimeEndian::Big] {
            let mut db = fixture(version, endian);
            assert!(db.children.is_empty());
            let variable = db.children(db.roots[0]).unwrap().remove(0);
            assert_eq!(variable.name, "head");
            assert_eq!(db.children.len(), 1);
            let attrs = db.attributes(variable.id).unwrap();
            assert_eq!(
                attrs
                    .iter()
                    .find(|attr| attr.name == "const_value")
                    .unwrap()
                    .value,
                "-7"
            );
            let reference = attrs.iter().find(|attr| attr.name == "type").unwrap();
            assert_eq!(reference.form, "ref_addr");
            let target = reference.target.unwrap();
            assert_eq!(target.unit, 1);
            assert_eq!(db.die(target).unwrap().name, "Node");
            assert_eq!(db.path(target).unwrap().len(), 3);
            // An ancestry scan does not eagerly materialize the destination tree.
            assert_eq!(db.children.len(), 1);
            assert!(db.children(variable.id).unwrap().is_empty());
        }
    }
}

#[test]
fn dwarf5_mixed_units_preserve_file_order_and_references() {
    // Interleave type and compilation units in .debug_info, starting with
    // a type unit as GCC can do with -gdwarf-5 -fdebug-types-section.
    let abbrev = [
        1, 0x41, 1, 0, 0, // type_unit with children
        2, 0x24, 0, 3, 8, 0, 0, // base_type with inline name
        3, 0x11, 0, 3, 8, 0, 0, // compile_unit with inline name
        0,
    ];
    let mut info = Vec::new();
    let mut starts = Vec::new();
    for (kind, name, signature) in [
        (gimli::DW_UT_type, "int", Some(11u64)),
        (gimli::DW_UT_compile, "first.c", None),
        (gimli::DW_UT_split_type, "char", Some(22u64)),
        (gimli::DW_UT_compile, "second.c", None),
    ] {
        let start = info.len();
        starts.push(start);
        info.extend_from_slice(&0u32.to_le_bytes());
        info.extend_from_slice(&5u16.to_le_bytes());
        info.extend_from_slice(&[kind.0, 8]);
        info.extend_from_slice(&0u32.to_le_bytes());
        if let Some(signature) = signature {
            info.extend_from_slice(&signature.to_le_bytes());
            info.extend_from_slice(&25u32.to_le_bytes());
            info.extend_from_slice(&[1, 2]);
        } else {
            info.push(3);
        }
        info.extend_from_slice(name.as_bytes());
        info.push(0);
        if signature.is_some() {
            info.push(0);
        }
        let length = (info.len() - start - 4) as u32;
        info[start..start + 4].copy_from_slice(&length.to_le_bytes());
    }
    let dwarf = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes: &[u8] = match id {
            gimli::SectionId::DebugInfo => &info,
            gimli::SectionId::DebugAbbrev => &abbrev,
            _ => &[],
        };
        Ok(RelocateReader::new(
            EndianRcSlice::new(Rc::from(bytes), RunTimeEndian::Little),
            Relocations(Rc::new(object::RelocationMap::default())),
        ))
    })
    .unwrap();
    let mut db = Database::from_dwarf(dwarf).unwrap();
    assert_eq!(
        db.roots.iter().map(|id| id.unit).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    for (signature, unit, name) in [(11, 0, "int"), (22, 2, "char")] {
        let target = db
            .reference(
                1,
                Value::DebugTypesRef(gimli::DebugTypeSignature(signature)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(target, DieId { unit, offset: 25 });
        assert_eq!(db.die(target).unwrap().name, name);
        assert_eq!(db.path(target).unwrap()[0].unit, unit);
    }
    assert_eq!(
        db.reference(
            0,
            Value::DebugInfoRef(gimli::DebugInfoOffset(starts[3] + 12))
        )
        .unwrap(),
        Some(db.roots[3])
    );
}

pub fn imports_fixture(version: u16) -> Database {
    let abbrev = [
        1, 0x11, 1, 3, 8, 0, 0, // compile_unit with name and children
        2, 0x3c, 1, 3, 8, 0, 0, // partial_unit with name and children
        3, 0x3d, 0, 0x18, 0x10, 0, 0, // imported_unit: import / ref_addr
        4, 0x2e, 0, 3, 8, 0, 0, // subprogram with name
        0,
    ];
    let mut info = Vec::new();
    let mut roots = Vec::new();
    let mut imports = Vec::new();
    for index in 0..2 {
        let start = info.len();
        info.extend_from_slice(&0u32.to_le_bytes());
        info.extend_from_slice(&version.to_le_bytes());
        if version == 5 {
            info.extend_from_slice(&[
                if index == 0 {
                    gimli::DW_UT_compile.0
                } else {
                    gimli::DW_UT_partial.0
                },
                8,
            ]);
            info.extend_from_slice(&0u32.to_le_bytes());
        } else {
            info.extend_from_slice(&0u32.to_le_bytes());
            info.push(8);
        }
        roots.push(info.len());
        info.push(if index == 0 { 1 } else { 2 });
        info.extend_from_slice(if index == 0 { b"main.c\0" } else { b"shared\0" });
        if index == 0 {
            // Two occurrences of the same import, a broken import, and a self-import.
            for _ in 0..4 {
                imports.push(info.len());
                info.push(3);
                info.extend_from_slice(&0u32.to_le_bytes());
            }
        } else {
            info.push(4);
            info.extend_from_slice(b"helper\0");
            imports.push(info.len());
            info.push(3);
            info.extend_from_slice(&0u32.to_le_bytes());
        }
        info.push(0);
        let length = (info.len() - start - 4) as u32;
        info[start..start + 4].copy_from_slice(&length.to_le_bytes());
    }
    for (import, target) in
        imports
            .iter()
            .zip([roots[1], roots[1], u32::MAX as usize, imports[3], roots[0]])
    {
        info[import + 1..import + 5].copy_from_slice(&(target as u32).to_le_bytes());
    }
    let dwarf = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes: &[u8] = match id {
            gimli::SectionId::DebugInfo => &info,
            gimli::SectionId::DebugAbbrev => &abbrev,
            _ => &[],
        };
        Ok(RelocateReader::new(
            EndianRcSlice::new(Rc::from(bytes), RunTimeEndian::Little),
            Relocations(Rc::new(object::RelocationMap::default())),
        ))
    })
    .unwrap();
    Database::from_dwarf(dwarf).unwrap()
}

#[test]
fn import_targets_are_separate_from_physical_children_and_ancestry() {
    for version in [4, 5] {
        let mut db = imports_fixture(version);
        let imports = db.children(db.roots[0]).unwrap();
        assert_eq!(imports.len(), 4);
        assert!(
            imports[0]
                .import_label
                .as_ref()
                .unwrap()
                .ends_with("partial_unit shared")
        );
        assert!(!imports[0].has_children);
        assert!(db.children(imports[0].id).unwrap().is_empty());
        let target = db.imported_unit(imports[0].id).unwrap();
        assert_eq!(target.id, db.roots[1]);
        assert_eq!(target.tag, "partial_unit");
        assert_eq!(db.imported_unit(imports[1].id).unwrap().id, target.id);
        let children = db.children(target.id).unwrap();
        assert_eq!(children[0].name, "helper");
        assert_eq!(db.imported_unit(children[1].id).unwrap().id, db.roots[0]);
        assert_eq!(
            db.path(children[0].id).unwrap(),
            [target.id, children[0].id]
        );
        assert!(db.imported_unit(imports[2].id).is_err());
        assert_eq!(db.imported_unit(imports[3].id).unwrap().id, imports[3].id);
        assert_eq!(db.path(target.id).unwrap(), [target.id]);
    }
}

pub fn type_fixture() -> Database {
    // Build a real DWARF4 type unit containing a base type. Both sections begin
    // at zero, and the signature must resolve to the type DIE, not the root.
    let abbrev = [1, 0x41, 1, 0, 0, 2, 0x24, 0, 3, 8, 0, 0, 0];
    let signature: u64 = 0x123456789abcdef0;
    let mut types = Vec::new();
    types.extend_from_slice(&29u32.to_le_bytes());
    types.extend_from_slice(&4u16.to_le_bytes());
    types.extend_from_slice(&0u32.to_le_bytes());
    types.push(8);
    types.extend_from_slice(&signature.to_le_bytes());
    types.extend_from_slice(&24u32.to_le_bytes());
    types.extend_from_slice(&[1, 2, b'i', b'n', b't', 0, 0]);
    // Unit length excludes the initial four-byte length field.
    let length = (types.len() - 4) as u32;
    types[..4].copy_from_slice(&length.to_le_bytes());
    let dwarf = Dwarf::load(|id| -> gimli::Result<DwarfReader> {
        let bytes: &[u8] = match id {
            gimli::SectionId::DebugTypes => &types,
            gimli::SectionId::DebugAbbrev => &abbrev,
            _ => &[],
        };
        Ok(RelocateReader::new(
            EndianRcSlice::new(Rc::from(bytes), RunTimeEndian::Little),
            Relocations(Rc::new(object::RelocationMap::default())),
        ))
    })
    .unwrap();
    Database::from_dwarf(dwarf).unwrap()
}

#[test]
fn type_section_offsets_do_not_alias_info_offsets() {
    let mut db = fixture(4, RunTimeEndian::Little);
    let root = db.roots[0];
    let signature: u64 = 0x123456789abcdef0;
    let type_db = type_fixture();
    let index = db.units.len();
    db.roots.extend(type_db.roots.into_iter().map(|id| DieId {
        unit: id.unit + index,
        offset: id.offset,
    }));
    db.units.extend(type_db.units);
    let type_id = DieId {
        unit: index,
        offset: 24,
    };
    db.signatures.insert((false, signature), type_id);
    assert_eq!(db.die(type_id).unwrap().name, "int");
    assert!(db.location(type_id).starts_with("types:"));
    assert!(db.location(root).starts_with("info:"));
    assert_eq!(db.find_die(DieSection::Types, 24).unwrap(), type_id);
    assert!(db.find_die(DieSection::Info, 24).is_err());
    assert_eq!(
        db.reference(
            0,
            Value::DebugTypesRef(gimli::DebugTypeSignature(signature))
        )
        .unwrap(),
        Some(type_id)
    );
    assert!(
        db.reference(0, Value::DebugInfoRef(gimli::DebugInfoOffset(usize::MAX)))
            .is_err()
    );
}

#[test]
fn find_die_rejects_headers_attribute_bytes_nulls_and_out_of_bounds_offsets() {
    let mut db = fixture(5, RunTimeEndian::Little);
    let section = DieSection::Info;
    let root = db.roots[0];
    assert_eq!(db.find_die(section, root.offset).unwrap(), root);
    let second = db.roots[1];
    let second_offset = db.units[1].header.offset().0 + second.offset;
    assert_eq!(db.find_die(section, second_offset).unwrap(), second);
    let terminator = db.units[0].header.length_including_self() - 1;
    for offset in [0, root.offset + 1, terminator, usize::MAX] {
        assert!(db.find_die(section, offset).is_err(), "offset 0x{offset:x}");
    }
    assert!(db.find_die(DieSection::Types, root.offset).is_err());
}
