# Choose a local decision model

This example asks one Jevscript judgment through either [Laya](https://github.com/NandhaKishorM/laya) or [Kev](https://github.com/jaredpalmer/kev). Both projects provide a System One compatible `POST /v1/systemone` server. The same [`urgent.jev`](urgent.jev) program works with either server. Jevscript's `--model` option selects the entry in [`profiles.json`](profiles.json), which supplies the endpoint and request limits.

You need `jevscript` on your path. From a source checkout, build the binary with `cargo build -p jevscript-cli` and use `target/debug/jevscript` in place of `jevscript` below. Run the Jevscript commands from its repository root. Install each model server in a separate directory, then start it before running the judgment command.

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

For either server, the CLI prints `urgent` as a tagged `prob` value between 0 and 1. The value is the selected model's estimate for this question, so the two checkpoints need not return the same number.

## Local bearer tokens and limits

Jevscript's HTTP client always reads `TYPESAFE_API_KEY` and sends it as a bearer token. The local Laya and Kev servers allow requests without authentication by default, so `local` is a placeholder value. If you set `LAYA_API_KEY` or `KEV_API_KEY` on a server, set `TYPESAFE_API_KEY` to that same value in the terminal that runs `jevscript`. Do not put tokens in `profiles.json`.

The profile limits are conservative settings for this one-question example. They are not claims about either model's maximum context or tokenizer. `chars4` is Jevscript's rough size estimator. The zero price records no provider token charge for local inference; it does not account for hardware or electricity. Each provider can return different probabilities for the same question. Measure accuracy and calibrate thresholds on your own labelled cases before using a judgment to make consequential decisions.

For another local server that speaks the same System One request and response format, add a profile with its documented model ID and full `/v1/systemone` endpoint, then select it with `--model`. A generic OpenAI text-generation endpoint does not implement this format.
