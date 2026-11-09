"""Short-lived Parakeet worker (from the Python version): never downloads models or saves user content."""
import json
from pathlib import Path
import sys


def main():
    def offline(event, args):
        if event in ('socket.connect', 'socket.getaddrinfo', 'socket.bind'):
            raise RuntimeError('Networking is disabled during local transcription')
    sys.addaudithook(offline)
    import mlx.core as mx
    import mlx.nn as nn
    from parakeet_mlx.utils import from_config
    from parakeet_mlx.parakeet import DecodingConfig
    model_dir, audio = map(Path, sys.argv[1:])
    config = json.loads((model_dir / 'config.json').read_text())
    quant = config.pop('quantization')
    if quant.get('bits') != 4 or quant.get('group_size') != 64 or quant.get('mode', 'affine') != 'affine':
        raise ValueError('Configure the tested Parakeet v2 4-bit checkpoint')
    mx.set_memory_limit(3 * 1024 ** 3)
    mx.set_cache_limit(64 * 1024 ** 2)
    mx.random.seed(0)
    weights = mx.load(str(model_dir / 'model.safetensors'))
    model = from_config(config)
    nn.quantize(model, bits=4, group_size=64, mode='affine',
                class_predicate=lambda path, module: path + '.scales' in weights)
    model.load_weights(list(weights.items()), strict=True)
    model.eval()
    mx.eval(model.parameters())
    result = model.transcribe(str(audio), dtype=mx.bfloat16,
                              decoding_config=DecodingConfig(), chunk_duration=None)
    mx.synchronize()
    print(json.dumps({'text': result.text}, ensure_ascii=False))


if __name__ == '__main__':
    main()
