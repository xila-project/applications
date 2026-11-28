//! Back a guest's linear memory with one allocation for the lifetime of its store.

use alloc::{alloc::alloc_zeroed, boxed::Box, string::ToString, vec::Vec};
use core::{alloc::Layout, ptr::NonNull, slice};
use wasmi::{Engine, Linker, Memory, MemoryType, Module, Store, errors::MemoryError};
use wasmparser::{Chunk, Parser, Payload};

use super::error::{Error, Result};

const IMPORT_MODULE: &str = "xila";
const IMPORT_FIELD: &str = "linear_memory";
const MEMORY_IMPORT_PREFIX_LENGTH: usize = IMPORT_MODULE.len() + IMPORT_FIELD.len() + 3;

// The import entry plus up to a section ID and two unsigned 32-bit LEB128 values.
// The memory type itself moves from the old section into the new import.
pub(super) const SOURCE_HEADROOM: usize = MEMORY_IMPORT_PREFIX_LENGTH + 11;

/// Wasmi 2.0.2 can use static storage only for a memory created by the host.
/// Move the guest's memory declaration to an import without changing its
/// type, index, exports, or data segments. Edit the source buffer in place.
pub(super) fn compile(
    engine: &Engine,
    mut source: Vec<u8>,
    growth_allowance: usize,
) -> Result<(Module, Option<MemoryPlan>)> {
    let mut parser = Parser::new(0);
    let mut offset = 0;
    let mut import = None;
    let mut insert_at = 0;
    loop {
        let Chunk::Parsed { consumed, payload } = parser
            .parse(&source[offset..], true)
            .map_err(parser_error)?
        else {
            unreachable!("the full source is available");
        };
        let end = offset + consumed;
        match payload {
            Payload::ImportSection(section) => {
                import = Some((offset..section.original_position(), end, section.count()));
            }
            Payload::Version { .. } | Payload::TypeSection(_) => insert_at = end,
            Payload::MemorySection(section) if section.count() != 0 => {
                // Validate the original binary too. For example, moving the memory
                // could otherwise hide a duplicate or out-of-order section.
                Module::validate(engine, &source)?;
                let type_start = section.original_position();
                let memory = section.into_iter().next().unwrap().map_err(parser_error)?;
                let plan = MemoryPlan::new(memory, growth_allowance)?;
                let mut entry = Vec::with_capacity(MEMORY_IMPORT_PREFIX_LENGTH + end - type_start);
                for name in [IMPORT_MODULE, IMPORT_FIELD] {
                    entry.push(name.len() as u8);
                    entry.extend_from_slice(name.as_bytes());
                }
                entry.push(2); // memory import
                entry.extend_from_slice(&source[type_start..end]);

                source
                    .try_reserve_exact(SOURCE_HEADROOM)
                    .map_err(|_| memory_error(MemoryError::OutOfSystemMemory))?;
                drop(source.drain(offset..end));
                let (header_range, entries_end, count) =
                    import.unwrap_or((insert_at..insert_at, insert_at, 0));
                let entries_length = entries_end - header_range.end;
                let mut header = Vec::new();
                header.push(2); // import section
                let (encoded_count, count_length) = leb128fmt::encode_u32(count + 1).unwrap();
                let length = u32::try_from(count_length + entries_length + entry.len())
                    .map_err(|_| wasmi::Error::new("WebAssembly import section is too large"))?;
                let (encoded_length, length_length) = leb128fmt::encode_u32(length).unwrap();
                header.extend_from_slice(&encoded_length[..length_length]);
                header.extend_from_slice(&encoded_count[..count_length]);

                drop(source.splice(entries_end..entries_end, entry));
                drop(source.splice(header_range, header));
                return Ok((Module::new(engine, &source)?, Some(plan)));
            }
            Payload::CodeSectionStart { .. } | Payload::End(_) => {
                return Ok((Module::new(engine, &source)?, None));
            }
            _ => {}
        }
        offset = end;
    }
}

fn parser_error(error: wasmparser::BinaryReaderError) -> Error {
    wasmi::Error::new(error.to_string()).into()
}

fn memory_error(error: MemoryError) -> Error {
    wasmi::Error::from(error).into()
}

pub(super) struct MemoryPlan {
    memory_type: MemoryType,
    capacity: usize,
}

impl MemoryPlan {
    fn new(memory: wasmparser::MemoryType, growth_allowance: usize) -> Result<Self> {
        let page_size_log2 = memory.page_size_log2.unwrap_or(16) as u8;
        let page_size = 1u128 << page_size_log2;
        let (maximum, capacity) = match memory.maximum {
            Some(maximum) => (maximum, u128::from(maximum) * page_size),
            None => {
                let address_space = 1u128 << if memory.memory64 { 64 } else { 32 };
                let maximum_pages = (address_space / page_size).min(address_space - 1);
                let capacity = (u128::from(memory.initial) * page_size + growth_allowance as u128)
                    .min(maximum_pages * page_size);
                ((capacity / page_size) as u64, capacity)
            }
        };
        let mut builder = MemoryType::builder();
        builder
            .min(memory.initial)
            .max(Some(maximum))
            .memory64(memory.memory64)
            .page_size_log2(page_size_log2);
        let memory_type = builder.build().map_err(memory_error)?;
        let capacity =
            usize::try_from(capacity).map_err(|_| memory_error(MemoryError::OutOfSystemMemory))?;
        Ok(Self {
            memory_type,
            capacity,
        })
    }
}

/// Declare the buffer before the store so it outlives every Wasmi memory handle.
pub(super) struct MemoryBuffer {
    bytes: NonNull<[u8]>,
    memory_type: MemoryType,
}

impl MemoryBuffer {
    pub(super) fn allocate(plan: MemoryPlan) -> Result<Self> {
        let layout = Layout::array::<u8>(plan.capacity)
            .map_err(|_| memory_error(MemoryError::OutOfSystemMemory))?;
        let pointer = if layout.size() == 0 {
            NonNull::dangling()
        } else {
            // SAFETY: The layout has a nonzero size. The global allocator
            // prefers PSRAM for large blocks on Espressif boards that have it.
            NonNull::new(unsafe { alloc_zeroed(layout) })
                .ok_or_else(|| memory_error(MemoryError::OutOfSystemMemory))?
        };
        Ok(Self {
            bytes: NonNull::slice_from_raw_parts(pointer, layout.size()),
            memory_type: plan.memory_type,
        })
    }

    /// # Safety
    ///
    /// Call once per buffer. Drop the store and linker before dropping the buffer.
    pub(super) unsafe fn link<T>(
        &mut self,
        store: &mut Store<T>,
        linker: &mut Linker<T>,
    ) -> Result<()> {
        // SAFETY: The allocation is initialized and exclusive. Wasmi keeps
        // its raw pointer in the store and does not free the allocation.
        let bytes = unsafe {
            slice::from_raw_parts_mut(self.bytes.cast::<u8>().as_ptr(), self.bytes.len())
        };
        let memory = Memory::new_static(store, self.memory_type, bytes)?;
        linker.define(IMPORT_MODULE, IMPORT_FIELD, memory)?;
        Ok(())
    }
}

impl Drop for MemoryBuffer {
    fn drop(&mut self) {
        // SAFETY: Allocation used the global allocator and the layout of the
        // slice. The caller has dropped the store, so no access can overlap.
        drop(unsafe { Box::from_raw(self.bytes.as_ptr()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Translation, engine_configuration};
    use alloc::format;

    fn check_growth(
        declaration: &str,
        allowance: usize,
        initial: i32,
        growth: i32,
        capacity: usize,
        with_import: bool,
    ) {
        let import = if with_import {
            r#"(import "host" "called" (func $called)) (start $called)"#
        } else {
            ""
        };
        let source = wat::parse_str(format!(
            r#"(module {import}
                (memory (export "memory") {declaration})
                (data (i32.const 0) "x")
                (func (export "grow") (param i32) (result i32)
                    local.get 0 memory.grow))"#
        ))
        .unwrap();
        for translation in [Translation::Eager, Translation::Lazy] {
            let engine = Engine::new(&engine_configuration(translation));
            let (module, plan) = compile(&engine, source.clone(), allowance).unwrap();
            let mut buffer = MemoryBuffer::allocate(plan.unwrap()).unwrap();
            assert_eq!(buffer.bytes.len(), capacity);
            let mut store = Store::new(&engine, false);
            let mut linker = Linker::new(&engine);
            if with_import {
                linker
                    .func_wrap("host", "called", |mut caller: wasmi::Caller<'_, bool>| {
                        *caller.data_mut() = true;
                    })
                    .unwrap();
            }
            // SAFETY: Link once. The store and linker drop before the buffer.
            unsafe { buffer.link(&mut store, &mut linker) }.unwrap();
            store.set_fuel(10_000).unwrap();
            let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
            assert_eq!(*store.data(), with_import);
            let memory = instance.get_memory(&store, "memory").unwrap();
            let grow = instance.get_typed_func::<i32, i32>(&store, "grow").unwrap();
            let pointer = memory.data(&store).as_ptr();
            assert_eq!(memory.size(&store), initial as u64);
            assert_eq!(grow.call(&mut store, growth).unwrap(), initial);
            assert_eq!(grow.call(&mut store, 1).unwrap(), -1);
            assert_eq!(memory.size(&store), (initial + growth) as u64);
            let data = memory.data(&store);
            assert_eq!(data.as_ptr(), pointer);
            assert_eq!(data[0], b'x');
            assert!(data[1..].iter().all(|&byte| byte == 0));
        }
    }

    #[test]
    fn declared_maximum_sets_the_exact_capacity() {
        check_growth("12 16 (pagesize 1)", 64 * 1024, 12, 4, 16, true);
    }

    #[test]
    fn no_maximum_uses_the_allowance_in_bytes() {
        check_growth("12 (pagesize 1)", 20, 12, 20, 32, false);
        check_growth("1", 65_537, 1, 1, 131_073, true);
    }

    #[test]
    fn memory_is_optional_and_invalid_section_order_is_rejected() {
        let engine = Engine::new(&engine_configuration(Translation::Eager));
        let no_memory = wat::parse_str(r#"(module (func (export "_start")))"#).unwrap();
        assert!(compile(&engine, no_memory, 0).unwrap().1.is_none());

        let mut invalid = wat::parse_str(r#"(module (func (export "_start")))"#).unwrap();
        // An out-of-order memory section after the code section.
        invalid.extend_from_slice(&[5, 3, 1, 0, 0]);
        assert!(compile(&engine, invalid, 0).is_err());

        let mut invalid = wat::parse_str(r#"(module (memory (export "memory") 0))"#).unwrap();
        // Duplicate, empty memory section after the export section.
        invalid.extend_from_slice(&[5, 1, 0]);
        assert!(compile(&engine, invalid, 0).is_err());
    }
}
