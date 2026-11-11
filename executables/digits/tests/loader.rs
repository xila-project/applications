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
