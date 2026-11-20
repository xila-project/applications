pub use crate::FunctionCall;
pub use crate::{EVENT_CLOSE_REQUESTED, EventCode};
pub use core::ffi::c_void;

#[link(wasm_import_module = "host")]
unsafe extern "C" {
    #[link_name = "xila_graphics_call"]
    pub fn xila_graphics_call(
        function: FunctionCall,
        argument_0: usize,
        argument_1: usize,
        argument_2: usize,
        argument_3: usize,
        argument_4: usize,
        argument_5: usize,
        argument_6: usize,
        argument_count: u8,
        return_value: *mut core::ffi::c_void,
    ) -> i32;
}

// The guest-visible types are shared with Core's native graphics bindings.
include!(concat!(env!("OUT_DIR"), "/types.generated.rs"));

/// Writes the result directly to the guest address supplied by the caller.
#[inline]
pub(super) unsafe fn call_raw(
    function: FunctionCall,
    arguments: &[usize],
    result: *mut c_void,
) -> i32 {
    let mut words = [0; 7];
    words[..arguments.len()].copy_from_slice(arguments);
    let [a0, a1, a2, a3, a4, a5, a6] = words;
    unsafe {
        xila_graphics_call(
            function,
            a0,
            a1,
            a2,
            a3,
            a4,
            a5,
            a6,
            arguments.len() as u8,
            result,
        )
    }
}

/// Call a registry function whose result type permits an all-zero value.
#[inline]
pub(super) unsafe fn call<T>(function: FunctionCall, arguments: &[usize]) -> Result<T> {
    let mut result: T = unsafe { core::mem::zeroed() };
    let result_pointer = if size_of::<T>() == 0 {
        core::ptr::null_mut()
    } else {
        (&raw mut result).cast()
    };
    let status = unsafe { call_raw(function, arguments, result_pointer) };
    if status == 0 { Ok(result) } else { Err(status) }
}

pub use super::enumeration::{
    BlurQuality, STATE_ANY, STATE_CHECKED, STATE_DEFAULT, STATE_DISABLED, STATE_EDITED,
    STATE_FOCUS_KEY, STATE_FOCUSED, STATE_HOVERED, STATE_PRESSED, STATE_SCROLLED, STATE_USER_1,
    STATE_USER_2, STATE_USER_3, STATE_USER_4, State, TextLeadingTrim,
};

#[cfg(test)]
mod chart_type_tests {
    use super::ChartType;

    #[test]
    fn chart_types_match_lvgl_values() {
        // lv_chart_type_t: NONE, LINE, CURVE, BAR, STACKED, SCATTER.
        assert_eq!(ChartType::None as u32, 0);
        assert_eq!(ChartType::Line as u32, 1);
        assert_eq!(ChartType::Curve as u32, 2);
        assert_eq!(ChartType::Bar as u32, 3);
        assert_eq!(ChartType::Stacked as u32, 4);
        assert_eq!(ChartType::Scatter as u32, 5);
    }
}
