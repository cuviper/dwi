use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result, bail, ensure};
use gimli::{
    AttributeValue as Value, DebuggingInformationEntry, Dwarf, EndianRcSlice, Reader,
    RelocateReader, RunTimeEndian, Unit, UnitOffset, UnitType,
};
use object::{Object, ObjectSection};

mod load;
pub use load::LoadLimits;

#[derive(Clone, Debug)]
struct Relocations(Rc<object::RelocationMap>);

impl gimli::Relocate for Relocations {
    fn relocate_address(&self, offset: usize, value: u64) -> gimli::Result<u64> {
        Ok(self.0.relocate(offset as u64, value))
    }

    fn relocate_offset(&self, offset: usize, value: usize) -> gimli::Result<usize> {
        usize::try_from(self.0.relocate(offset as u64, value as u64))
            .map_err(|_| gimli::Error::UnsupportedOffset)
    }
}

type DwarfReader = RelocateReader<EndianRcSlice<RunTimeEndian>, Relocations>;
type Entry = DebuggingInformationEntry<DwarfReader>;

/// Unit indices are unique across sections and main/supplementary objects.
/// Never use a bare section offset as identity: these address spaces overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DieId {
    pub unit: usize,
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DieSection {
    Info,
    Types,
    SupplementaryInfo,
    SupplementaryTypes,
}

impl DieSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Types => "types",
            Self::SupplementaryInfo => "sup:info",
            Self::SupplementaryTypes => "sup:types",
        }
    }

    fn supplementary(self) -> bool {
        matches!(self, Self::SupplementaryInfo | Self::SupplementaryTypes)
    }

    fn section_id(self) -> gimli::SectionId {
        match self {
            Self::Info | Self::SupplementaryInfo => gimli::SectionId::DebugInfo,
            Self::Types | Self::SupplementaryTypes => gimli::SectionId::DebugTypes,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Die {
    pub id: DieId,
    pub tag: String,
    pub name: String,
    pub location: String,
    pub has_children: bool,
    pub import_label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub owner: DieId,
    pub name: String,
    pub form: String,
    pub value: String,
    pub is_expression: bool,
    pub raw: String,
    pub target: Option<DieId>,
    pub location_list: Option<LocationListRef>,
    pub range_list: Option<RangeListRef>,
    pub sibling: bool,
}

impl Attribute {
    pub fn has_list(&self) -> bool {
        self.location_list.is_some() || self.range_list.is_some()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum LocationListRef {
    Offset(gimli::LocationListsOffset<usize>),
    Index(gimli::DebugLocListsIndex<usize>),
}

impl LocationListRef {
    fn value(self) -> Value<DwarfReader> {
        match self {
            Self::Offset(offset) => Value::LocationListsRef(offset),
            Self::Index(index) => Value::DebugLocListsIndex(index),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RangeListRef {
    Offset(gimli::RawRangeListsOffset<usize>),
    Index(gimli::DebugRngListsIndex<usize>),
}

impl RangeListRef {
    fn value(self) -> Value<DwarfReader> {
        match self {
            Self::Offset(offset) => Value::RangeListsRef(offset),
            Self::Index(index) => Value::DebugRngListsIndex(index),
        }
    }
}

pub struct Database {
    dwarf: Dwarf<DwarfReader>,
    units: Vec<Unit<DwarfReader>>,
    pub roots: Vec<DieId>,
    pub debug_file: Option<PathBuf>,
    pub supplementary_file: Option<PathBuf>,
    pub supplementary_error: Option<String>,
    supplementary_start: Option<usize>,
    signatures: HashMap<(bool, u64), DieId>,
    summaries: HashMap<DieId, Die>,
    children: HashMap<DieId, Vec<DieId>>,
    parents: HashMap<DieId, DieId>,
}

impl Database {
    pub fn open(path: &Path, limits: LoadLimits) -> Result<Self> {
        load::open(path, limits)
    }

    pub fn open_kernel(release: Option<&str>, limits: LoadLimits) -> Result<(Self, PathBuf)> {
        load::open_kernel(release, limits)
    }

    pub fn open_module(
        name: &str,
        release: Option<&str>,
        limits: LoadLimits,
    ) -> Result<(Self, PathBuf)> {
        load::open_module(name, release, limits)
    }

    fn dwarf_from_object(
        object: &object::File<'_>,
        path: &Path,
        loader: &load::SectionLoader,
    ) -> Result<Dwarf<DwarfReader>> {
        let endian = if object.is_little_endian() {
            RunTimeEndian::Little
        } else {
            RunTimeEndian::Big
        };
        let dwarf = Dwarf::load(|id| -> Result<DwarfReader> {
            let section = object.section_by_name(id.name()).or_else(|| {
                // GNU-style compression predates SHF_COMPRESSED.
                object.section_by_name(&id.name().replacen(".debug_", ".zdebug_", 1))
            });
            let (data, relocations) = match section {
                Some(section) => (
                    loader.load_section(&section, path)?,
                    section
                        .relocation_map()
                        .with_context(|| format!("reading relocations for {}", id.name()))?,
                ),
                None => (Rc::from([]), object::RelocationMap::default()),
            };
            Ok(RelocateReader::new(
                EndianRcSlice::new(data, endian),
                Relocations(Rc::new(relocations)),
            ))
        })?;
        Ok(dwarf)
    }

    fn append_units(dwarf: &Dwarf<DwarfReader>, units: &mut Vec<Unit<DwarfReader>>) -> Result<()> {
        let mut headers = dwarf.units();
        while let Some(header) = headers.next().context("reading .debug_info headers")? {
            units.push(dwarf.unit(header).context("reading .debug_info unit")?);
        }
        let mut headers = dwarf.type_units();
        while let Some(header) = headers.next().context("reading .debug_types headers")? {
            units.push(dwarf.unit(header).context("reading .debug_types unit")?);
        }
        Ok(())
    }

    fn from_dwarf(dwarf: Dwarf<DwarfReader>) -> Result<Self> {
        let mut units = Vec::new();
        Self::append_units(&dwarf, &mut units)?;
        let supplementary_start = if let Some(sup) = dwarf.sup() {
            let start = units.len();
            Self::append_units(sup, &mut units).context("reading supplementary units")?;
            Some(start)
        } else {
            None
        };
        ensure!(
            !units.is_empty(),
            "no DWARF units found in .debug_info or .debug_types"
        );
        let mut roots = Vec::new();
        let mut signatures = HashMap::new();
        for (index, unit) in units.iter().enumerate() {
            if let Some(entry) = unit.entries().next_dfs()? {
                roots.push(DieId {
                    unit: index,
                    offset: entry.offset().0,
                });
            }
            if let UnitType::Type {
                type_signature,
                type_offset,
            }
            | UnitType::SplitType {
                type_signature,
                type_offset,
            } = unit.header.type_()
            {
                signatures.insert(
                    (
                        supplementary_start.is_some_and(|start| index >= start),
                        type_signature.0,
                    ),
                    DieId {
                        unit: index,
                        offset: type_offset.0,
                    },
                );
            }
        }
        Ok(Self {
            dwarf,
            units,
            roots,
            debug_file: None,
            supplementary_file: None,
            supplementary_error: None,
            supplementary_start,
            signatures,
            summaries: HashMap::new(),
            children: HashMap::new(),
            parents: HashMap::new(),
        })
    }

    fn is_supplementary(&self, unit: usize) -> bool {
        self.supplementary_start.is_some_and(|start| unit >= start)
    }

    fn dwarf_for_unit(&self, unit: usize) -> &Dwarf<DwarfReader> {
        if self.is_supplementary(unit) {
            self.dwarf
                .sup()
                .expect("supplementary units have an owning object")
        } else {
            &self.dwarf
        }
    }

    fn require_supplementary(&self, unit: usize) -> Result<()> {
        ensure!(
            !self.is_supplementary(unit),
            "a supplementary object cannot reference another supplementary object"
        );
        ensure!(
            self.dwarf.sup().is_some(),
            "{}",
            self.supplementary_error.as_deref().unwrap_or(
                "supplementary debug object is not loaded (no supplementary-file link was found)"
            )
        );
        Ok(())
    }

    pub fn section(&self, id: DieId) -> DieSection {
        match (
            self.is_supplementary(id.unit),
            self.units[id.unit].header.section(),
        ) {
            (false, gimli::SectionId::DebugTypes) => DieSection::Types,
            (false, _) => DieSection::Info,
            (true, gimli::SectionId::DebugTypes) => DieSection::SupplementaryTypes,
            (true, _) => DieSection::SupplementaryInfo,
        }
    }

    pub fn find_die(&mut self, section: DieSection, offset: usize) -> Result<DieId> {
        let id = self
            .units
            .iter()
            .enumerate()
            .find_map(|(unit, data)| {
                if data.header.section() != section.section_id()
                    || self.is_supplementary(unit) != section.supplementary()
                {
                    return None;
                }
                gimli::UnitSectionOffset(offset)
                    .to_unit_offset(&data.header)
                    .map(|offset| DieId {
                        unit,
                        offset: offset.0,
                    })
            })
            .with_context(|| format!("No unit contains {}:0x{offset:x}", section.label()))?;
        // Reading a DIE at an arbitrary byte offset can accidentally parse an
        // attribute's bytes. Walking from the root verifies a real DIE boundary.
        self.path(id)?;
        Ok(id)
    }

    pub fn location(&self, id: DieId) -> String {
        let header = &self.units[id.unit].header;
        let section = self.section(id).label();
        format!(
            "{section}:0x{:x}",
            header.offset().0 as u128 + id.offset as u128
        )
    }

    fn entry(&self, id: DieId) -> Result<Entry> {
        let entry = self.units[id.unit]
            .entry(UnitOffset(id.offset))
            .with_context(|| format!("reading {}", self.location(id)))?;
        ensure!(
            !entry.is_null(),
            "{} points to a null DIE",
            self.location(id)
        );
        Ok(entry)
    }

    fn summarize(&self, unit: usize, entry: &Entry) -> Die {
        let id = DieId {
            unit,
            offset: entry.offset().0,
        };
        let name = self
            .die_name(unit, entry)
            .unwrap_or_else(|error| format!("<name: {error}>"));
        let import_label = if entry.tag() == gimli::DW_TAG_imported_unit {
            entry.attr_value(gimli::DW_AT_import).map(|value| {
                self.describe_import(unit, value)
                    .unwrap_or_else(|error| format!("<unresolved: {error}>"))
            })
        } else {
            None
        };
        Die {
            id,
            tag: short(&entry.tag().to_string(), "DW_TAG_"),
            name,
            location: self.location(id),
            has_children: entry.has_children(),
            import_label,
        }
    }

    fn describe_import(&self, unit: usize, value: Value<DwarfReader>) -> Result<String> {
        let target = self
            .reference(unit, value)?
            .context("DW_AT_import is not a DIE reference")?;
        let entry = self.entry(target)?;
        // Describe only this target, without recursively following its imports.
        let name = self
            .die_name(target.unit, &entry)
            .unwrap_or_else(|error| format!("<name: {error}>"));
        Ok(format!(
            "[{}] {} {}",
            self.location(target),
            short(&entry.tag().to_string(), "DW_TAG_"),
            name
        )
        .trim_end()
        .to_owned())
    }

    fn die_name(&self, mut unit: usize, entry: &Entry) -> Result<String> {
        let mut linkage = None;
        let name = (|| -> Result<Option<String>> {
            let mut current = Cow::Borrowed(entry);
            let mut visited = HashSet::new();
            loop {
                if let Some(value) = current.attr_value(gimli::DW_AT_name) {
                    return self.string(unit, value).map(Some);
                }
                // An inherited source name is preferable to a local mangled name.
                // Save the closest linkage name, including its decoding context.
                if linkage.is_none() {
                    linkage = current
                        .attr_value(gimli::DW_AT_linkage_name)
                        .or_else(|| current.attr_value(gimli::DW_AT_MIPS_linkage_name))
                        .map(|value| (unit, value));
                }
                let Some(reference) = current
                    .attr_value(gimli::DW_AT_abstract_origin)
                    .or_else(|| current.attr_value(gimli::DW_AT_specification))
                else {
                    return Ok(None);
                };
                ensure!(
                    visited.insert(DieId {
                        unit,
                        offset: current.offset().0
                    }),
                    "cycle in name origin/specification chain"
                );
                let target = self
                    .reference(unit, reference)?
                    .context("name origin/specification is not a DIE reference")?;
                current = Cow::Owned(self.entry(target)?);
                unit = target.unit;
            }
        })();
        match (name, linkage) {
            (Ok(Some(name)), _) => Ok(name),
            // Keep a usable linkage name even if an origin is unavailable.
            (_, Some((unit, value))) => self.string(unit, value),
            (Ok(None), None) => Ok(String::new()),
            (Err(error), None) => Err(error),
        }
    }

    pub fn die(&mut self, id: DieId) -> Result<Die> {
        if let Some(die) = self.summaries.get(&id) {
            return Ok(die.clone());
        }
        let die = self.summarize(id.unit, &self.entry(id)?);
        self.summaries.insert(id, die.clone());
        Ok(die)
    }

    pub fn imported_unit(&mut self, id: DieId) -> Result<Die> {
        let value = self
            .entry(id)?
            .attr_value(gimli::DW_AT_import)
            .context("imported unit has no DW_AT_import")?;
        let target = self
            .reference(id.unit, value)?
            .context("DW_AT_import is not a DIE reference")?;
        self.die(target)
    }

    pub fn children(&mut self, id: DieId) -> Result<Vec<Die>> {
        if let Some(children) = self.children.get(&id).cloned() {
            return children.into_iter().map(|id| self.die(id)).collect();
        }
        let mut result = Vec::new();
        let mut cursor = self.units[id.unit].entries_at_offset(UnitOffset(id.offset))?;
        if cursor.next_dfs()?.is_some_and(|entry| entry.has_children()) {
            cursor.next_entry()?;
            while let Some(entry) = cursor.current() {
                result.push(self.summarize(id.unit, entry));
                cursor.next_sibling()?;
            }
        }
        for child in &result {
            self.parents.insert(child.id, id);
            self.summaries.insert(child.id, child.clone());
        }
        self.children
            .insert(id, result.iter().map(|die| die.id).collect());
        Ok(result)
    }

    /// Scan only the destination unit when ancestry is not yet known. This reveals
    /// references across CUs without expanding unrelated trees or caching every DIE.
    pub fn path(&mut self, id: DieId) -> Result<Vec<DieId>> {
        let mut path = vec![id];
        while let Some(parent) = self.parents.get(path.last().unwrap()) {
            path.push(*parent);
        }
        if self.roots.contains(path.last().unwrap()) {
            path.reverse();
            return Ok(path);
        }
        path.clear();
        let mut cursor = self.units[id.unit].entries();
        while let Some(entry) = cursor.next_dfs()? {
            ensure!(entry.depth() >= 0, "invalid DIE depth");
            path.truncate(entry.depth() as usize);
            let current = DieId {
                unit: id.unit,
                offset: entry.offset().0,
            };
            path.push(current);
            if current == id {
                for pair in path.windows(2) {
                    self.parents.insert(pair[1], pair[0]);
                }
                return Ok(path);
            }
        }
        bail!("{} is not a DIE in its unit", self.location(id))
    }

    fn reference(&self, unit: usize, value: Value<DwarfReader>) -> Result<Option<DieId>> {
        Ok(match value {
            Value::UnitRef(offset) => Some(DieId {
                unit,
                offset: offset.0,
            }),
            Value::DebugInfoRef(offset) => Some(
                self.units
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        self.is_supplementary(*index) == self.is_supplementary(unit)
                    })
                    .find_map(|(unit, data)| {
                        offset.to_unit_offset(&data.header).map(|offset| DieId {
                            unit,
                            offset: offset.0,
                        })
                    })
                    .context("reference is outside .debug_info units")?,
            ),
            Value::DebugTypesRef(signature) => Some(
                *self
                    .signatures
                    .get(&(self.is_supplementary(unit), signature.0))
                    .context("type signature is not present in this file")?,
            ),
            Value::DebugInfoRefSup(offset) => {
                self.require_supplementary(unit)?;
                Some(
                    self.units
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| self.is_supplementary(*index))
                        .find_map(|(unit, data)| {
                            offset.to_unit_offset(&data.header).map(|offset| DieId {
                                unit,
                                offset: offset.0,
                            })
                        })
                        .context("reference is outside supplementary .debug_info units")?,
                )
            }
            _ => None,
        })
    }

    pub fn attributes(&mut self, id: DieId) -> Result<Vec<Attribute>> {
        let entry = self.entry(id)?;
        let mut result = Vec::new();
        for attr in entry.attrs() {
            let value = attr.value();
            let is_expression = matches!(value, Value::Exprloc(_));
            let location_list = match &value {
                Value::LocationListsRef(offset) => Some(LocationListRef::Offset(*offset)),
                Value::DebugLocListsIndex(index) => Some(LocationListRef::Index(*index)),
                _ => None,
            };
            let range_list = match &value {
                Value::RangeListsRef(offset) => Some(RangeListRef::Offset(*offset)),
                Value::DebugRngListsIndex(index) => Some(RangeListRef::Index(*index)),
                _ => None,
            };
            let reference = self.reference(id.unit, value.clone());
            let (text, target) = match reference {
                Ok(Some(target)) => match self.die(target) {
                    Ok(die) => (
                        format!("→ [{}] {} {}", die.location, die.tag, die.name)
                            .trim_end()
                            .to_owned(),
                        Some(target),
                    ),
                    Err(error) => (format!("<unresolved: {error}>"), None),
                },
                Ok(None) => (
                    self.format_value(id.unit, value)
                        .unwrap_or_else(|error| format!("<unresolved: {error}>")),
                    None,
                ),
                Err(error) => (format!("<unresolved: {error}>"), None),
            };
            // Byte/string readers have huge Debug implementations; explicitly format
            // those values to preserve useful raw data without dumping backing sections.
            let raw = match attr.raw_value() {
                Value::String(reader) => format!("{:?}", reader.to_string_lossy()?),
                Value::Block(reader) => hex_bytes(&reader)?,
                Value::Exprloc(expr) => hex_bytes(&expr.0)?,
                other => format!("{other:?}"),
            };
            result.push(Attribute {
                owner: id,
                name: short(&attr.name().to_string(), "DW_AT_"),
                form: short(&attr.form().to_string(), "DW_FORM_"),
                value: text,
                is_expression,
                raw,
                target,
                location_list,
                range_list,
                sibling: attr.name() == gimli::DW_AT_sibling,
            });
        }
        Ok(result)
    }

    pub fn locations(&self, owner: DieId, list: LocationListRef) -> Result<Vec<Attribute>> {
        let unit = &self.units[owner.unit];
        let mut locations = self
            .dwarf_for_unit(owner.unit)
            .attr_locations(unit, list.value())?
            .context("attribute is not a location list")?;
        let mut result = Vec::new();
        // Keep the raw entry's kind so a default location is distinguishable
        // from an explicit address range. Gimli resolves bases and address indices.
        while let Some(raw) = locations.next_raw()? {
            let default = matches!(raw, gimli::RawLocListEntry::DefaultLocation { .. });
            let Some(location) = locations.convert_raw(raw)? else {
                continue;
            };
            let name = if default {
                "default".into()
            } else {
                format!("[0x{:x}, 0x{:x})", location.range.begin, location.range.end)
            };
            let raw = hex_bytes(&location.data.0)?;
            let value = if location.data.0.is_empty() {
                "<unavailable>".into()
            } else {
                format_expression(location.data, unit.encoding())?
            };
            result.push(Attribute {
                owner,
                name,
                form: String::new(),
                value,
                is_expression: true,
                raw,
                target: None,
                location_list: None,
                range_list: None,
                sibling: false,
            });
        }
        Ok(result)
    }

    pub fn ranges(&self, owner: DieId, list: RangeListRef) -> Result<Vec<Attribute>> {
        let unit = &self.units[owner.unit];
        let mut ranges = self
            .dwarf_for_unit(owner.unit)
            .attr_ranges(unit, list.value())?
            .context("attribute is not a range list")?;
        let mut result = Vec::new();
        while let Some(range) = ranges.next()? {
            result.push(Attribute {
                owner,
                name: format!("[0x{:x}, 0x{:x})", range.begin, range.end),
                form: String::new(),
                value: String::new(),
                is_expression: false,
                raw: String::new(),
                target: None,
                location_list: None,
                range_list: None,
                sibling: false,
            });
        }
        Ok(result)
    }

    fn string(&self, unit: usize, value: Value<DwarfReader>) -> Result<String> {
        if matches!(value, Value::DebugStrRefSup(_)) {
            self.require_supplementary(unit)?;
        }
        Ok(self
            .dwarf_for_unit(unit)
            .attr_string(&self.units[unit], value)?
            .to_string_lossy()?
            .into_owned())
    }

    fn format_value(&self, unit: usize, value: Value<DwarfReader>) -> Result<String> {
        Ok(match value {
            Value::String(_)
            | Value::DebugStrRef(_)
            | Value::DebugLineStrRef(_)
            | Value::DebugStrRefSup(_)
            | Value::DebugStrOffsetsIndex(_) => format!("{:?}", self.string(unit, value)?),
            Value::Addr(address) => format!("0x{address:x}"),
            Value::DebugAddrIndex(_) => self
                .dwarf_for_unit(unit)
                .attr_address(&self.units[unit], value)?
                .map(|address| format!("0x{address:x}"))
                .context("missing indexed address")?,
            Value::Data1(n) => n.to_string(),
            Value::Data2(n) => n.to_string(),
            Value::Data4(n) => n.to_string(),
            Value::Data8(n) => n.to_string(),
            Value::Data16(n) => format!("0x{n:x}"),
            Value::Sdata(n) => n.to_string(),
            Value::Udata(n) => n.to_string(),
            Value::Flag(flag) => flag.to_string(),
            Value::Encoding(n) => n.to_string(),
            Value::Language(n) => n.to_string(),
            Value::Accessibility(n) => n.to_string(),
            Value::Visibility(n) => n.to_string(),
            Value::Virtuality(n) => n.to_string(),
            Value::Inline(n) => n.to_string(),
            Value::CallingConvention(n) => n.to_string(),
            Value::Ordering(n) => n.to_string(),
            Value::DecimalSign(n) => n.to_string(),
            Value::Endianity(n) => n.to_string(),
            Value::IdentifierCase(n) => n.to_string(),
            Value::AddressClass(n) => n.to_string(),
            Value::FileIndex(index) => self
                .file_name(unit, index)
                .map(|name| format!("{name} [file {index}]"))
                .unwrap_or_else(|| format!("file {index} (unresolved)")),
            Value::Block(reader) => hex_bytes(&reader)?,
            Value::Exprloc(expr) => format_expression(expr, self.units[unit].encoding())?,
            Value::LocationListsRef(offset) => {
                let section = if self.units[unit].header.version() < 5 {
                    ".debug_loc"
                } else {
                    ".debug_loclists"
                };
                format!("{section}+0x{:x}", offset.0)
            }
            Value::DebugLocListsIndex(index) => format!(".debug_loclists index {}", index.0),
            Value::RangeListsRef(gimli::RawRangeListsOffset(offset))
            | Value::DebugRngListsBase(gimli::DebugRngListsBase(offset)) => {
                let section = if self.units[unit].header.version() < 5 {
                    ".debug_ranges"
                } else {
                    ".debug_rnglists"
                };
                format!("{section}+0x{offset:x}")
            }
            Value::DebugRngListsIndex(index) => format!(".debug_rnglists index {}", index.0),
            Value::DebugAddrBase(offset) => format!(".debug_addr+0x{:x}", offset.0),
            Value::DebugStrOffsetsBase(offset) => format!(".debug_str_offsets+0x{:x}", offset.0),
            Value::DebugLocListsBase(offset) => format!(".debug_loclists+0x{:x}", offset.0),
            Value::DebugMacinfoRef(offset) => format!(".debug_macinfo+0x{:x}", offset.0),
            Value::DebugMacroRef(offset) => format!(".debug_macro+0x{:x}", offset.0),
            Value::DwoId(id) => format!("0x{:016x}", id.0),
            Value::DebugLineRef(offset) => format!(".debug_line+0x{:x}", offset.0),
            Value::SecOffset(offset) => format!("section offset 0x{offset:x}"),
            other => format!("{other:?}"),
        })
    }

    fn file_name(&self, unit: usize, index: u64) -> Option<String> {
        let unit_data = &self.units[unit];
        let header = unit_data.line_program.as_ref()?.header();
        let file = header.file(index)?;
        let name = self.string(unit, file.path_name()).ok()?;
        let mut path = std::path::PathBuf::new();
        if let Some(comp_dir) = &unit_data.comp_dir {
            path.push(comp_dir.to_string_lossy().ok()?.as_ref());
        }
        if let Some(directory) = file.directory(header) {
            path.push(self.string(unit, directory).ok()?);
        }
        path.push(name);
        Some(path.display().to_string())
    }
}

fn short(value: &str, prefix: &str) -> String {
    value.strip_prefix(prefix).unwrap_or(value).to_owned()
}

fn format_expression(
    expr: gimli::Expression<DwarfReader>,
    encoding: gimli::Encoding,
) -> Result<String> {
    format_expression_inner(expr.0, encoding, &mut 32)
}

fn format_expression_inner(
    mut input: DwarfReader,
    encoding: gimli::Encoding,
    remaining: &mut usize,
) -> Result<String> {
    let mut instructions = Vec::new();
    // Share the budget with nested entry-value expressions, bounding both output
    // length and recursion depth.
    while !input.is_empty() && *remaining > 0 {
        *remaining -= 1;
        let original = input.clone();
        // Preserve the encoded opcode: Operation normalizes e.g. reg7 and regx.
        let opcode = gimli::DwOp(input.clone().read_u8()?);
        let mut instruction = opcode.to_string();
        match gimli::Operation::parse(&mut input, encoding) {
            Ok(operation) => {
                let operands = format_operands(opcode, operation, encoding, remaining)?;
                if !operands.is_empty() {
                    instruction.push(' ');
                    instruction.push_str(&operands);
                }
            }
            Err(error) => {
                instructions.push(format!(
                    "{instruction} <decode error: {error}>; raw {}",
                    hex_bytes(&original)?
                ));
                return Ok(instructions.join("; "));
            }
        }
        instructions.push(instruction);
    }
    if !input.is_empty() {
        instructions.push("…".into());
    }
    Ok(instructions.join("; "))
}

fn format_operands(
    opcode: gimli::DwOp,
    operation: gimli::Operation<DwarfReader>,
    encoding: gimli::Encoding,
    remaining: &mut usize,
) -> Result<String> {
    use gimli::Operation as Op;

    Ok(match operation {
        Op::Deref {
            size, base_type, ..
        } => match opcode {
            gimli::DW_OP_deref | gimli::DW_OP_xderef => String::new(),
            gimli::DW_OP_deref_size | gimli::DW_OP_xderef_size => size.to_string(),
            _ => format!("{size} type=cu+0x{:x}", base_type.0),
        },
        Op::Pick { index } => {
            if opcode == gimli::DW_OP_pick {
                index.to_string()
            } else {
                String::new()
            }
        }
        Op::UnsignedConstant { value } => {
            if (gimli::DW_OP_lit0.0..=gimli::DW_OP_lit31.0).contains(&opcode.0) {
                String::new()
            } else {
                value.to_string()
            }
        }
        Op::SignedConstant { value } => value.to_string(),
        Op::PlusConstant { value } => value.to_string(),
        Op::Bra { target } | Op::Skip { target } => format!("{target:+}"),
        Op::Register { register } => {
            if opcode == gimli::DW_OP_regx {
                register.0.to_string()
            } else {
                String::new()
            }
        }
        Op::RegisterOffset {
            register,
            offset,
            base_type,
        } => match opcode {
            gimli::DW_OP_bregx => format!("{} {offset}", register.0),
            gimli::DW_OP_regval_type | gimli::DW_OP_GNU_regval_type => {
                format!("{} type=cu+0x{:x}", register.0, base_type.0)
            }
            _ => offset.to_string(),
        },
        Op::FrameOffset { offset } => offset.to_string(),
        Op::Call { offset } => match offset {
            gimli::DieReference::UnitRef(offset) => format!("cu+0x{:x}", offset.0),
            gimli::DieReference::DebugInfoRef(offset) => format!("info:0x{:x}", offset.0),
        },
        Op::VariableValue { offset } => format!("info:0x{:x}", offset.0),
        Op::Piece {
            size_in_bits,
            bit_offset,
        } => match bit_offset {
            Some(offset) => format!("{size_in_bits} bits, offset {offset} bits"),
            None => {
                let size = size_in_bits / 8;
                format!("{size} {}", if size == 1 { "byte" } else { "bytes" })
            }
        },
        Op::ImplicitValue { data } => hex_bytes(&data)?,
        Op::ImplicitPointer { value, byte_offset } => format!("info:0x{:x} {byte_offset}", value.0),
        Op::EntryValue { expression } => {
            format!(
                "({})",
                format_expression_inner(expression, encoding, remaining)?
            )
        }
        Op::ParameterRef { offset } => format!("cu+0x{:x}", offset.0),
        Op::Address { address } => format!("0x{address:x}"),
        Op::AddressIndex { index } | Op::ConstantIndex { index } => index.0.to_string(),
        Op::TypedLiteral { base_type, value } => {
            format!("type=cu+0x{:x} {}", base_type.0, hex_bytes(&value)?)
        }
        Op::Convert { base_type } | Op::Reinterpret { base_type } => {
            format!("type=cu+0x{:x}", base_type.0)
        }
        Op::WasmLocal { index } => format!("local {index}"),
        Op::WasmGlobal { index } => format!("global {index}"),
        Op::WasmStack { index } => format!("stack {index}"),
        Op::Drop
        | Op::Swap
        | Op::Rot
        | Op::Abs
        | Op::And
        | Op::Div
        | Op::Minus
        | Op::Mod
        | Op::Mul
        | Op::Neg
        | Op::Not
        | Op::Or
        | Op::Plus
        | Op::Shl
        | Op::Shr
        | Op::Shra
        | Op::Xor
        | Op::Eq
        | Op::Ge
        | Op::Gt
        | Op::Le
        | Op::Lt
        | Op::Ne
        | Op::Nop
        | Op::PushObjectAddress
        | Op::TLS
        | Op::CallFrameCFA
        | Op::StackValue
        | Op::Uninitialized => String::new(),
    })
}

fn hex_bytes(reader: &DwarfReader) -> Result<String> {
    let bytes = reader.to_slice()?;
    let mut text = bytes
        .iter()
        .take(256)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > 256 {
        text.push_str(" …");
    }
    let unit = if bytes.len() == 1 { "byte" } else { "bytes" };
    Ok(format!("[{text}] ({} {unit})", bytes.len()))
}

#[cfg(test)]
pub(crate) mod tests;
