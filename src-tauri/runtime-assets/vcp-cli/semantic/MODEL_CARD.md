# Frozen local semantic model

VCP Mobile ships a compact static embedding snapshot derived upstream as
[`Nourh7/granite-embedding-97m-multilingual-r2`](https://huggingface.co/Nourh7/granite-embedding-97m-multilingual-r2),
revision `b77044bfd84eef0b552c5346eeacc851264592b3`.

The upstream model card identifies it as a Model2Vec distillation of
`ibm-granite/granite-embedding-97m-multilingual-r2`, declares the MIT license, and describes it as
a multilingual static embedding model. VCP Mobile does not ship Python, Model2Vec, a generic
tokenizer runtime, or a network inference client. It ships only:

- the frozen `model.safetensors` bytes;
- a deterministic compact ByteLevel BPE pack generated from the frozen `tokenizer.json`;
- a Rust mmap reader that applies weighted mean pooling, excludes padding, and L2 normalizes the
  resulting 64-dimensional vector.

Exact bytes, hashes, dimensions and source identities live in
[`../semantic-profile.json`](../semantic-profile.json). The compact pack is a mechanical encoding
of the upstream tokenizer vocabulary and merges; it is not a newly trained model.

The model is used only for on-device `river=semantic:N`. Source query text is not persisted or
logged. Cached vectors are device-local, derived, disposable, and absent from sync.
