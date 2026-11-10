#!/usr/bin/env python3
"""Train/export the small MNIST classifier. Requires numpy, flatbuffers, tflite.

Run with OPENBLAS_NUM_THREADS=2 for reproducible bounded CPU use.
The downloaded dataset stays in the supplied cache, outside the repository.
"""
import argparse
import hashlib
from pathlib import Path
import urllib.request

import flatbuffers
import numpy as np
import tflite as t


def export(weights, destination):
    b = flatbuffers.Builder(120000)

    def ints(values):
        return b.CreateNumpyVector(np.array(values, dtype='<i4'))

    def offsets(values):
        b.StartVector(4, len(values), 4)
        for value in reversed(values):
            b.PrependUOffsetTRelative(value)
        return b.EndVector()

    buffers = []
    for data in [None, *weights]:
        payload = b.CreateByteVector(data.astype('<f4').tobytes()) if data is not None else None
        t.BufferStart(b)
        if payload is not None:
            t.BufferAddData(b, payload)
        buffers.append(t.BufferEnd(b))
    tensors = []
    # FC weights are output-major, matching TFLite/Nanoglia.
    for name, shape, buffer in [('input', [1, 784], 0), ('w1', [32, 784], 1),
                                 ('b1', [32], 2), ('hidden', [1, 32], 0),
                                 ('w2', [10, 32], 3), ('b2', [10], 4),
                                 ('logits', [1, 10], 0), ('scores', [1, 10], 0)]:
        name, shape = b.CreateString(name), ints(shape)
        t.TensorStart(b)
        t.TensorAddName(b, name)
        t.TensorAddShape(b, shape)
        t.TensorAddType(b, t.TensorType.FLOAT32)
        t.TensorAddBuffer(b, buffer)
        tensors.append(t.TensorEnd(b))
    codes = []
    for code in [t.BuiltinOperator.FULLY_CONNECTED, t.BuiltinOperator.SOFTMAX]:
        t.OperatorCodeStart(b)
        t.OperatorCodeAddDeprecatedBuiltinCode(b, code)
        t.OperatorCodeAddBuiltinCode(b, code)
        t.OperatorCodeAddVersion(b, 1)
        codes.append(t.OperatorCodeEnd(b))
    ops = []
    for inputs, outputs, activation in [([0, 1, 2], [3], t.ActivationFunctionType.RELU),
                                         ([3, 4, 5], [6], t.ActivationFunctionType.NONE)]:
        t.FullyConnectedOptionsStart(b)
        t.FullyConnectedOptionsAddFusedActivationFunction(b, activation)
        options = t.FullyConnectedOptionsEnd(b)
        inputs, outputs = ints(inputs), ints(outputs)
        t.OperatorStart(b)
        t.OperatorAddOpcodeIndex(b, 0)
        t.OperatorAddInputs(b, inputs)
        t.OperatorAddOutputs(b, outputs)
        t.OperatorAddBuiltinOptionsType(b, t.BuiltinOptions.FullyConnectedOptions)
        t.OperatorAddBuiltinOptions(b, options)
        ops.append(t.OperatorEnd(b))
    t.SoftmaxOptionsStart(b)
    t.SoftmaxOptionsAddBeta(b, 1.0)
    options = t.SoftmaxOptionsEnd(b)
    inputs, outputs = ints([6]), ints([7])
    t.OperatorStart(b)
    t.OperatorAddOpcodeIndex(b, 1)
    t.OperatorAddInputs(b, inputs)
    t.OperatorAddOutputs(b, outputs)
    t.OperatorAddBuiltinOptionsType(b, t.BuiltinOptions.SoftmaxOptions)
    t.OperatorAddBuiltinOptions(b, options)
    ops.append(t.OperatorEnd(b))
    tensors, operators, inputs, outputs = offsets(tensors), offsets(ops), ints([0]), ints([7])
    t.SubGraphStart(b)
    t.SubGraphAddTensors(b, tensors)
    t.SubGraphAddOperators(b, operators)
    t.SubGraphAddInputs(b, inputs)
    t.SubGraphAddOutputs(b, outputs)
    graph = t.SubGraphEnd(b)
    graphs, codes, buffers = offsets([graph]), offsets(codes), offsets(buffers)
    description = b.CreateString('Xila Digits: MNIST 784-32-10 ReLU/Softmax, seed 42')
    t.ModelStart(b)
    t.ModelAddVersion(b, 3)
    t.ModelAddSubgraphs(b, graphs)
    t.ModelAddOperatorCodes(b, codes)
    t.ModelAddBuffers(b, buffers)
    t.ModelAddDescription(b, description)
    b.Finish(t.ModelEnd(b), file_identifier=b'TFL3')
    destination.write_bytes(b.Output())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=Path, required=True)
    args = parser.parse_args()
    args.cache.mkdir(parents=True, exist_ok=True)
    dataset = args.cache / 'mnist.npz'
    if not dataset.exists():
        urllib.request.urlretrieve('https://storage.googleapis.com/tensorflow/tf-keras-datasets/mnist.npz', dataset)
    digest = hashlib.sha256(dataset.read_bytes()).hexdigest()
    if digest != '731c5ac602752760c8e48fbffcf8c3b850d9dc2a2aedcf2cc48468fc17b673d1':
        raise ValueError('MNIST dataset checksum mismatch')
    data = np.load(dataset, allow_pickle=False)
    x = data['x_train'].reshape(-1, 784).astype('float32') / 255
    y = data['y_train']
    xt = data['x_test'].reshape(-1, 784).astype('float32') / 255
    yt = data['y_test']
    rng = np.random.default_rng(42)
    w1 = (rng.standard_normal((784, 32)) * np.sqrt(2 / 784)).astype('float32')
    b1 = np.zeros(32, dtype='float32')
    w2 = (rng.standard_normal((32, 10)) * np.sqrt(2 / 32)).astype('float32')
    b2 = np.zeros(10, dtype='float32')
    weights = [w1, b1, w2, b2]
    m, v = [np.zeros_like(w) for w in weights], [np.zeros_like(w) for w in weights]
    step = 0
    for epoch in range(15):
        order = rng.permutation(len(x))
        for start in range(0, len(x), 128):
            ids = order[start:start + 128]
            batch = x[ids]
            h = np.maximum(batch @ w1 + b1, 0)
            logits = h @ w2 + b2
            p = np.exp(logits - logits.max(axis=1, keepdims=True))
            p /= p.sum(axis=1, keepdims=True)
            p[np.arange(len(ids)), y[ids]] -= 1
            p /= len(ids)
            dh = (p @ w2.T) * (h > 0)
            grads = [batch.T @ dh, dh.sum(axis=0), h.T @ p, p.sum(axis=0)]
            step += 1
            for w, grad, mean, variance in zip(weights, grads, m, v):
                mean *= 0.9
                mean += 0.1 * grad
                variance *= 0.999
                variance += 0.001 * grad * grad
                w -= 0.001 * (mean / (1 - 0.9**step)) / (np.sqrt(variance / (1 - 0.999**step)) + 1e-8)
        predicted = (np.maximum(xt @ w1 + b1, 0) @ w2 + b2).argmax(axis=1)
        accuracy = float((predicted == yt).mean())
        print(f'Epoch {epoch + 1}: test accuracy {accuracy:.4f}', flush=True)
    destination = Path(__file__).with_name('mnist.tflite')
    export([w1.T, b1, w2.T, b2], destination)
    # One real held-out sample per class, for backend regression tests.
    samples = np.stack([data['x_test'][np.flatnonzero(yt == digit)[0]] for digit in range(10)])
    destination.with_name('samples.bin').write_bytes(samples.tobytes())
    destination.with_name('training.txt').write_text(f'Dataset SHA-256: {digest}\nModel SHA-256: {hashlib.sha256(destination.read_bytes()).hexdigest()}\nMNIST test accuracy: {accuracy:.4f} (10000 held-out images)\n')


if __name__ == '__main__':
    main()
