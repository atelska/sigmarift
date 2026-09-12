# Models

SigmaRift does not distribute model weights in its Git repository. Download a
supported GGUF file and place it directly in this directory.

## Recommended model

Download [`gemma-4-E2B-it-Q4_K_M.gguf`](https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_K_M.gguf?download=true)
from [Unsloth's Gemma 4 E2B GGUF repository](https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF).
This is SigmaRift's default and recommended general-purpose model.

## Optional model

Download [`gemma-4-E4B-it-ultra-uncensored-heretic-Q5_K_M.gguf`](https://huggingface.co/llmfan46/gemma-4-E4B-it-ultra-uncensored-heretic-GGUF/resolve/main/gemma-4-E4B-it-ultra-uncensored-heretic-Q5_K_M.gguf?download=true)
from [its upstream repository](https://huggingface.co/llmfan46/gemma-4-E4B-it-ultra-uncensored-heretic-GGUF).

This model is explicitly described upstream as uncensored and decensored. Use
it only when that behavior is intended and appropriate for the environment.

## Terms

Each model has its own license and terms. Both sources identify their models as
Gemma 4 derivatives; review the [Gemma 4 license](https://ai.google.dev/gemma/docs/gemma_4_license)
and the model card before downloading or distributing a model.
