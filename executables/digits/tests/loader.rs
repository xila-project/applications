use digits::{LOADER_PATH, MODEL_BYTES};

#[test]
fn loader_path_matches_the_platform_node() {
    assert_eq!(LOADER_PATH, "/devices/nn/cpu");
}

#[test]
fn embedded_model_is_a_tensorflow_lite_flatbuffer() {
    // TensorFlow Lite FlatBuffers carry the "TFL3" identifier at offset 4.
    assert!(MODEL_BYTES.len() > 8);
    assert_eq!(&MODEL_BYTES[4..8], b"TFL3");
}

#[test]
fn backend_node_path_matches_the_platform_node() {
    assert_eq!(digits::BACKEND_PATH, "/devices/nn/backend");
}

#[test]
fn backend_label_accepts_a_backend_list() {
    assert_eq!(
        digits::backend_label(b"cmsis-nn,kernels"),
        Some("cmsis-nn,kernels")
    );
    assert_eq!(digits::backend_label(b"kernels\n"), Some("kernels"));
}

#[test]
fn backend_label_rejects_unusable_content() {
    assert_eq!(digits::backend_label(b""), None);
    assert_eq!(digits::backend_label(b"\n"), None);
    assert_eq!(digits::backend_label(b"two words"), None);
    assert_eq!(digits::backend_label(&[0xff, 0xfe]), None);
    assert_eq!(digits::backend_label(&[b'a'; 65]), None);
    assert_eq!(digits::backend_label(b"bell\x07"), None);
}
