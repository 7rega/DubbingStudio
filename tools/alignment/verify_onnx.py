"""Development-only: verify the published FP32 ONNX model against WhisperX.

Usage: python verify_onnx.py --reference <first-experiment> --audio <vocals.wav>
                           --output <empty-or-existing-work-directory>
Requires torch, torchaudio, whisperx, soundfile, onnxruntime, requests and blake3.
No application/project files are modified.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import onnxruntime as ort
import requests
import soundfile as sf
import torch
import torchaudio
from whisperx.alignment import get_trellis, backtrack, merge_repeats

REVISION = "a19f851b3d42865797e410752b4c570c871e4825"
BASE = f"https://huggingface.co/Xenova/wav2vec2-base-960h/resolve/{REVISION}"
SHA256 = "e46614273f03ff4b87923a965e417fa72004825522cb007c9c25633b8475490d"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--weights", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    for remote, local in [("onnx/model.onnx", "model.onnx"), ("vocab.json", "vocab.json"), ("config.json", "config.json"), ("preprocessor_config.json", "preprocessor_config.json")]:
        path = args.output / local
        if not path.exists():
            with requests.get(f"{BASE}/{remote}", stream=True, timeout=120) as response:
                response.raise_for_status()
                with path.with_suffix(".part").open("wb") as f:
                    for chunk in response.iter_content(1024*1024):
                        f.write(chunk)
            path.with_suffix(".part").replace(path)
    with (args.output/"model.onnx").open("rb") as f:
        assert hashlib.file_digest(f, "sha256").hexdigest() == SHA256
    vocab = {c.lower(): i for c,i in json.loads((args.output/"vocab.json").read_text()).items()}
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = 6
    session = ort.InferenceSession(str(args.output/"model.onnx"), opts, providers=["CPUExecutionProvider"])
    print("Inputs:", [(i.name, i.shape, i.type) for i in session.get_inputs()], flush=True)
    torch.set_num_threads(6)
    bundle = torchaudio.pipelines.WAV2VEC2_ASR_BASE_960H
    model = bundle.get_model(dl_kwargs={"model_dir":str(args.weights)}).eval()
    labels = bundle.get_labels()
    permutation = [vocab[c.lower()] if c.lower() in vocab else 0 for c in labels]
    samples, sr = sf.read(args.audio, dtype="float32")
    assert sr == 16000
    reports = []
    for path in sorted(args.reference.glob("block-*.json")):
        block = json.loads(path.read_text(encoding="utf-8"))
        t1,t2 = block["start"],block["end"]
        wave = samples[int(t1*sr):int(t2*sr)][None,:]
        feeds = {i.name: (wave if i.type=="tensor(float)" else np.ones(wave.shape,dtype=np.int64)) for i in session.get_inputs()}
        logits = session.run(None, feeds)[0][:,:,permutation]
        with torch.inference_mode():
            expected = model(torch.from_numpy(wave))[0].numpy()
        delta = float(np.max(np.abs(logits-expected)))
        emission = torch.log_softmax(torch.from_numpy(logits[0]),dim=-1)
        dictionary = {c.lower():i for i,c in enumerate(labels)}
        text = block["text"].lower().replace(" ","|")
        wildcard = emission[:,1:].max(dim=1).values
        emission = torch.cat([emission,wildcard[:,None]],dim=1)
        tokens = [dictionary.get(c,emission.shape[1]-1) for c in text]
        trellis = get_trellis(emission,tokens,0)
        aligned = merge_repeats(backtrack(trellis,emission,tokens,0),text)
        chars = [c for s in block["result"]["segments"] for c in s["chars"]]
        assert len(chars)==len(aligned)
        errors = []
        for old,new in zip(chars,aligned):
            if old["char"].isascii() and old["char"].isalpha():
                for key,frame in [("start",new.start),("end",new.end)]:
                    errors.append(abs(old[key]-round(t1+frame*(t2-t1)/logits.shape[1],3)))
        report = {"block":path.name,"max_logit_error":delta,"max_char_time_error_ms":round(max(errors)*1000,3)}
        reports.append(report)
        print(report,flush=True)
    (args.output/"parity.json").write_text(json.dumps(reports,indent=2),encoding="utf-8")
    assert max(r["max_char_time_error_ms"] for r in reports)<=25, "Parity gate failed"
    print("PASS: published FP32 ONNX timing parity")
    import blake3
    for name in ("model.onnx", "vocab.json", "config.json", "preprocessor_config.json"):
        path = args.output/name
        print(name, path.stat().st_size, blake3.blake3(path.read_bytes()).hexdigest())


if __name__=="__main__":
    main()
