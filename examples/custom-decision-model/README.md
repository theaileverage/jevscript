# Choose a local decision model

This example asks one Jevscript judgment through [Laya](https://github.com/NandhaKishorM/laya), [Kev](https://github.com/jaredpalmer/kev), [OpenJev weights on Hugging Face](https://huggingface.co/openjev/openjev), or the separate [OpenJev server on GitHub](https://github.com/razorback16/openjev). Each serves a System One compatible `POST /v1/systemone` endpoint. The same [`urgent.jev`](urgent.jev) program works with each server. Jevscript's `--model` option selects an entry in [`profiles.json`](profiles.json), which supplies the endpoint and request limits.

You need `jevscript` on your path. From a source checkout, build the binary with `cargo build -p jevscript-cli` and use `target/debug/jevscript` in place of `jevscript` below. Run the Jevscript commands from its repository root. Install each model server in a separate directory, then start it before running the judgment command. The two OpenJev projects use different weights and ports; their profile keys select different endpoints.

## Laya

The [Laya installation and server instructions](https://github.com/NandhaKishorM/laya#self-hosting-http-server-jev-compatible) require Python 3.10 or newer. In a directory you choose for Laya, these documented settings bind the server to loopback, use the CPU, and load the selected checkpoint on the first request:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install 'laya[serve]'
LAYA_HOST=127.0.0.1 LAYA_DEVICE=cpu LAYA_PRELOAD=0 laya-serve
```

In another terminal, from the Jevscript repository root:

```sh
TYPESAFE_API_KEY=local jevscript judge examples/custom-decision-model/urgent.jev classify \
  --state '{"message":"I need help before my meeting starts in an hour"}' \
  --model english --profiles examples/custom-decision-model/profiles.json
```

`english` selects Laya's English checkpoint. Laya documents `multilingual` and `typed-decisions` as other checkpoint identifiers. Add a separate profile before selecting one of those; the example profile is sized for this short English question.

## Kev

The [Kev local server instructions](https://github.com/jaredpalmer/kev#run-it-locally) require Python 3.12 or 3.13 and `uv`. The repo documents `kev-0.8b` for an Apple Silicon Mac or an L4 GPU. Its `--run` option accepts a Hugging Face checkpoint. Clone Kev from a directory outside the Jevscript checkout:

```sh
git clone https://github.com/jaredpalmer/kev.git
cd kev
uv sync --extra serve
uv run --extra serve python -m kev.serve --run jaredpalmer/kev-0.8b --host 127.0.0.1 --port 8009
```

In another terminal, from the Jevscript repository root:

```sh
TYPESAFE_API_KEY=local jevscript judge examples/custom-decision-model/urgent.jev classify \
  --state '{"message":"I need help before my meeting starts in an hour"}' \
  --model kev-latest --profiles examples/custom-decision-model/profiles.json
```

`kev-latest` is one of the local Kev server's accepted model IDs. `--run jaredpalmer/kev-0.8b` chooses the weights that server loads. Kev downloads the checkpoint and its base model on first use.

## OpenJev weights on Hugging Face

The [OpenJev model card](https://huggingface.co/openjev/openjev) and [serving guide](https://huggingface.co/openjev/openjev/blob/main/serve/SERVE.md) give this vLLM plus helper setup. The documented online FP8 recipe uses an H100-class GPU with 80 GB of memory. The weights are CC BY-NC 4.0, for non-commercial use with attribution; the helper and serving code are Apache-2.0. In a separate directory with Python and a supported NVIDIA setup, download the model and start vLLM:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install 'vllm==0.29.0' 'openai==3.16.2' 'httpx==0.28.1'
hf download openjev/openjev --local-dir openjev
vllm serve ./openjev --host 127.0.0.1 --served-model-name qwen --port 8000 \
  --enable-prefix-caching --max-model-len 16384 --gpu-memory-utilization 0.90 \
  --limit-mm-per-prompt '{"image":1}' --trust-remote-code --max-num-seqs 256 \
  --max-logprobs 64 --gdn-prefill-backend triton --quantization fp8
```

In another terminal in that directory, activate the same environment and start the decision API helper:

```sh
. .venv/bin/activate
VLLM=http://localhost:8000/v1 TOKENIZER=./openjev \
READOUT_T=0.85 READOUT_NOUL_T=1.829074 READOUT_NOUL_BIAS=0 \
READOUT_TARGETED=1 READOUT_INSTR_STYLE=pyrepr SHIM_STAGGER=1 \
python openjev/helper/shim.py --host 127.0.0.1 --port 3000
```

The `openjev` profile key selects the helper at `127.0.0.1:3000`. The helper ignores the request's `model` field and returns a `model` string that identifies the served directory, calibration settings, flags, and helper hash, as described in its [serving guide](https://huggingface.co/openjev/openjev/blob/main/serve/SERVE.md). From the Jevscript repository root, run:

```sh
TYPESAFE_API_KEY=local jevscript judge examples/custom-decision-model/urgent.jev classify \
  --state '{"message":"I need help before my meeting starts in an hour"}' \
  --model openjev --profiles examples/custom-decision-model/profiles.json
```

## OpenJev server on GitHub

The [GitHub OpenJev server's local setup](https://github.com/razorback16/openjev#run-your-own) runs a different model, DiffusionGemma 26B-A4B. Its server and weights are Apache-2.0. The documented Docker setup needs an NVIDIA GPU with at least 24 GB of memory. In a separate directory, run:

```sh
git clone https://github.com/razorback16/openjev.git
cd openjev
docker compose up -d openjev
curl http://127.0.0.1:8080/v1/models
```

The server listens on `127.0.0.1:8080` when the model has loaded. From the Jevscript repository root, run:

```sh
TYPESAFE_API_KEY=local jevscript judge examples/custom-decision-model/urgent.jev classify \
  --state '{"message":"I need help before my meeting starts in an hour"}' \
  --model openjev-latest --profiles examples/custom-decision-model/profiles.json
```

For all four servers, the CLI prints `urgent` as a tagged `prob` value between 0 and 1. The value is the selected model's estimate, so the checkpoints need not return the same number.

## Local bearer tokens and limits

Jevscript's HTTP client always reads `TYPESAFE_API_KEY` and sends it as a bearer token. The local Laya, Kev, and OpenJev servers allow requests without authentication by default, so `local` is a placeholder value. If you set `LAYA_API_KEY`, `KEV_API_KEY`, or the GitHub server's `OPENJEV_API_KEY`, set `TYPESAFE_API_KEY` to that same value in the terminal that runs `jevscript`. Do not put tokens in `profiles.json`.

The profile limits are conservative settings for this one-question example. They are not claims about a model's maximum context or tokenizer. `chars4` is Jevscript's rough size estimator. The zero price records no provider token charge for local inference; it does not account for hardware or electricity. Each provider can return different probabilities for the same question. Measure accuracy and calibrate thresholds on your own labelled cases before using a judgment to make consequential decisions.

Live inference through downloaded OpenJev weights was unavailable during preparation of this example. The CLI fixture checks selected profile routing, HTTP requests, and answer decoding against disposable servers; it does not establish live model inference or decision quality.

For another local server that speaks the same System One request and response format, add a profile with its documented model ID and full `/v1/systemone` endpoint, then select it with `--model`. A generic OpenAI text-generation endpoint does not implement this format.
