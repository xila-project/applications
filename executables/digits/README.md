# Digits

A small Xila Wasm application demonstrating the neural-network VFS abstraction.
Draw a digit from 0 to 9 with a mouse or touch input. Recognition runs when the
stroke ends, or with **Recognize**. **Undo** removes the last stroke; **Clear**
starts a new drawing. The three displayed scores are model probabilities, not
calibrated measures of correctness.

The canvas, bounded pixel storage, continuous strokes and viewport mapping reuse
Pixel Painter. Ink is cropped, area-resized into a 20×20 box, and centered in a
28×28 black frame. Empty drawings produce no prediction. The app uses one small
float32 MLP (784 → 32 ReLU → 10 Softmax); no camera, GPU, network or training
runtime is needed. Model accuracy on the unmodified MNIST test set does not
measure accuracy on freehand drawings.

## Build

```sh
mise run check:guest
mise run build:applications
cargo test -p xila-digits -p xila-pixel-painter
```

The guest is `target/wasm32-wasip1/release-wasm/digits.wasm`.
It imports Xila's graphics API and generic WASI `fd_control` extension; it needs
a Xila host, rather than running directly as a standalone browser module.

## Runtime contract

Digits opens the runtime's loader node `/devices/nn/cpu` read/write, writes its
embedded model and sends `LOAD`, then checks the input/output metadata, writes 784 little-endian floats,
issues `COMPUTE`, then reads 10 floats. Every inference seeks both input and output
to zero and rewrites the entire input. Closing the descriptor releases its arena.
The ABI identifiers and tensor layouts come from Core, not private copies.
The model must have one unquantized FLOAT32 input `[1,784]` and one FLOAT32 output
`[1,10]` containing softmax probabilities. Incompatible models and unavailable
NN devices show an error while leaving drawing and closing usable. Recognize
retries opening the device, so an unavailable backend can recover.

The model is embedded in the guest and loaded at runtime: Digits writes it to the
platform's loader node `/devices/nn/cpu` and sends `LOAD`, so any platform that
mounts a loader (Platforms' portable Nanoglia backend does) can run it. Hardware execution has not been
validated by these host tests.

For a local development image, build all guests and use Platforms' existing
`tools/bundle_applications.sh`. It includes Digits when `digits.wasm` is present;
the seed builder includes that local bundle without adding an unreleased package
to the published release lockfile. Launch **Digits** from the graphical shell.
See Platforms' `examples/native/README.md` for the complete development commands.

## Model provenance and regeneration

`models/train.py` trains from scratch with NumPy (seed 42, 15 Adam epochs, batches
of 128), then writes TFLite with FlatBuffers; TensorFlow is not required. Training
uses only the 60,000 training images. `models/training.txt` records model/dataset
SHA-256 and accuracy on the separate 10,000-image test set. Small numerical
variations between NumPy/BLAS implementations can change regenerated weights.

```sh
python3 -m venv /tmp/digits-training
/tmp/digits-training/bin/pip install numpy==2.2.4 flatbuffers==25.12.19 tflite==2.18.0
OPENBLAS_NUM_THREADS=2 /tmp/digits-training/bin/python \
  executables/digits/models/train.py --cache /tmp/mnist
```

The dataset is MNIST, by Yann LeCun, Corinna Cortes and Christopher J. C. Burges,
downloaded from the [Keras MNIST dataset mirror](https://storage.googleapis.com/tensorflow/tf-keras-datasets/mnist.npz).
The script verifies the dataset SHA-256 before training. `samples.bin` contains
one held-out image per class, in label order 0–9, for the platform backend
regression. The tiny model misclassifies the selected 5 as 8; the test retains
that limitation and compares with the NumPy oracle rather than selecting easier
examples. The trained weights and exporter are provided under this repository's
Apache-2.0 license; the MNIST samples retain their dataset provenance.
