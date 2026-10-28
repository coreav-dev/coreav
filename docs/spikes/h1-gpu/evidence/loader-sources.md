# ML video loader sampling parameters: verification against public sources

Researched 2026-10-13. Clean-room scope: only papers, official docs, model cards, release notes, configs
and the Python code in ML training repos that computes frame indices. I read no decoder or demuxer
implementation source (FFmpeg, PyAV, decord, torchcodec C++). The torchcodec findings come from its docs
and release notes only.

Pinned revisions (GitHub HEAD on 2026-10-13 unless a tag is given):

| Repo | Revision |
|---|---|
| MCG-NJU/VideoMAE | `14ef8d856287c94ef1f985fe30f958eb4ec2c55d` (2023-04-24) |
| facebookresearch/SlowFast | `287ec0076846560f44a9327e931a5a2360240533` |
| LLaVA-VL/LLaVA-NeXT | `bce12e479bc4dfee2b9c50c88137b01ff51bd483` |
| QwenLM/Qwen2.5-VL (now redirects to QwenLM/Qwen3-VL) | `vision_process.py` last changed in `fe120581464a989a8d0dcc3c0fc925e65f8b3ba1` (2025-11-17); PyPI `qwen-vl-utils` 0.0.8 / 0.0.11 / 0.0.14 also checked |
| huggingface/lerobot | `11c2480d5c79734b768da860b2eb2651941efb3c`; PyPI release `lerobot==0.6.1` (2026-08-03) cross-checked |
| hpcaitech/Open-Sora | `main` (Open-Sora 2.0 layout) |
| zai-org/CogVideo | `main` |
| Vchitect/Latte | `main` |
| torchcodec | docs at https://meta-pytorch.org/torchcodec/0.17/ ; release v0.17.0 (2026-09-30) |

Line numbers refer to those revisions.

---

## 1. Action recognition: "one 16-frame clip per video, every 4th frame (temporal stride 4)"

**Verdict: matches for VideoMAE on Kinetics-400. For SlowFast it matches only loosely. The default
SlowFast recipes use a 64-frame raw window: Slow takes 4x16 or 8x8 frames and Fast takes 32 frames at
stride 2. None of them uses 16 frames at stride 4. The evaluation protocol differs too: it takes
several clips per video.**

### VideoMAE (Tong et al., NeurIPS 2022, arXiv:2203.12602)

- Paper, sec. 3 "Temporal downsampling": "one video clip consisting of t consecutive frames is first
  randomly sampled from the original video V. We then use temporal sampling to compress the clip to T
  frames ... the stride τ is set to 4 and 2 on Kinetics and Something-Something, respectively."
  https://arxiv.org/pdf/2203.12602
- Paper, sec. 4 (datasets): "The Kinetics-400 contains around 240k training videos and 20k validation
  videos of 10s from 400 classes."
- Paper, Appendix, Kinetics-400: "During the fine-tuning phase, we perform the dense sampling following
  Slowfast ... For evaluation, all models share the same inference protocol, i.e., 5 clips × 3 crops."
  Table 8: input `3×16×224×224`, "stride 4×1×1 on K400".
- K400 pretraining script
  `scripts/kinetics/videomae_vit_base_patch16_224_tubemasking_ratio_0.9_epoch_1600/pretrain.sh` L17-18:
  `--num_frames 16`, `--sampling_rate 4`.
  https://github.com/MCG-NJU/VideoMAE/blob/main/scripts/kinetics/videomae_vit_base_patch16_224_tubemasking_ratio_0.9_epoch_1600/pretrain.sh
- K400 fine-tuning script, same dir, `finetune.sh`: L24 `--num_sample 2`, L25 `--input_size 224`,
  L26 `--short_side_size 224`, L28 `--num_frames 16`, L29 `--sampling_rate 4`,
  L36 `--test_num_segment 5`, L37 `--test_num_crop 3`.
- Data prep, `DATASET.md` L25: "Preprocess the dataset by resizing the short edge of video to
  **320px**." L41: all K400 experiments use the OpenDataLab copy with short edge 320px.
- How indices are computed (Python, `kinetics.py`):
  - Pretraining (`VideoMAE` class, built in `datasets.py` L36-52 with `new_length=num_frames`,
    `new_step=sampling_rate`, `num_segments` default 1, `temporal_jitter=False`).
    `skip_length = new_length*new_step = 64` (L455). `_sample_train_indices` (L515-536) picks one
    random offset in `[0, num_frames - 64]`. `_video_TSN_decord_batch_loader` (L539-552) takes
    `offset + 4*i` for i = 0..15. **This is exactly every 4th frame from a random start: 16 frames
    spanning 64 raw frames.**
  - Fine-tuning train mode (`VideoClsDataset.loadvideo_decord`, L252-267). `converted_len = 16*4 = 64`.
    `end_idx = np.random.randint(64, seg_len)`, `str_idx = end_idx - 64`, then
    `index = np.linspace(str_idx, end_idx, num=16)` cast to int. The effective spacing is 64/15, about
    4.27 frames, not exactly 4. Clips shorter than 64 frames get padded with the last frame. With
    `--num_sample 2` (L96-106), the frames are decoded once and augmented twice (repeated
    augmentation). The video is still decoded once per item.
  - Test mode (L243-248): decodes **every 4th frame of the whole video** with
    `range(0, len(vr), frame_sample_rate)`, then cuts 5 temporal windows of 16 and 3 spatial crops
    (L141-152).
  - Resolution: `datasets.py` L81-85 passes `keep_aspect_ratio=True`, so decoding happens at the stored
    resolution (short side 320). The frames are then resized and cropped to 224.

### SlowFast (Feichtenhofer et al., ICCV 2019, arXiv:1812.03982)

- Paper, sec. 3.1: "A typical value of τ we studied is 16 ... roughly 2 frames sampled per second for
  30-fps videos. Denoting the number of frames sampled by the Slow pathway as T, the raw clip length is
  T × τ frames." Sec. 3.2: "the Fast pathway samples αT frames ... A typical value is α = 8."
  Sec. 3.3: "T = 4 frames ... sparsely sampled from a 64-frame raw clip with a temporal stride τ = 16."
- Paper, sec. 4 "Training": "we randomly sample a clip (of αT ×τ frames) from the full-length video
  ... we randomly crop 224×224 pixels from a video, or its horizontal flip, with a shorter side randomly
  sampled in [256, 320] pixels." "Inference": "we uniformly sample 10 clips from a video along its
  temporal axis. For each clip, we scale the shorter spatial side to 256 pixels and take 3 crops of
  256×256" (30 views). "Kinetics-400 consists of ∼240k training videos and 20k validation videos."
- Config `configs/Kinetics/SLOWFAST_8x8_R50.yaml` L9-16: `NUM_FRAMES: 32`, `SAMPLING_RATE: 2`,
  `TRAIN_JITTER_SCALES: [256, 320]`, `TRAIN_CROP_SIZE: 224`, `TEST_CROP_SIZE: 256`, `ALPHA: 4`.
  The Fast pathway gets 32 frames at stride 2. The Slow pathway gets 32/4 = 8 frames at stride 8.
  Raw window: 64 frames.
  https://github.com/facebookresearch/SlowFast/blob/main/configs/Kinetics/SLOWFAST_8x8_R50.yaml
- Config `configs/Kinetics/SLOWFAST_4x16_R50.yaml` L9-16: same `NUM_FRAMES: 32`, `SAMPLING_RATE: 2`,
  scales and crops, but `ALPHA: 8`. The Slow pathway gets 4 frames at stride 16. Raw window: 64 frames.
- `slowfast/config/defaults.py`: L278 `_C.TEST.NUM_ENSEMBLE_VIEWS = 10`,
  L282 `_C.TEST.NUM_SPATIAL_CROPS = 3`, L666/669 defaults `NUM_FRAMES = 8`, `SAMPLING_RATE = 8`,
  L720 `TARGET_FPS = 30`, L726 `DECODING_BACKEND = "torchvision"`.
- Index computation, `slowfast/datasets/decoder.py`. L305-306:
  `clip_size = sampling_rate * num_frames / target_fps * fps`, so the window is rescaled to 30 fps.
  L59-74 `get_start_end_idx`: training (`clip_idx == -1`) uses `start_idx = random.uniform(0, delta)`.
  Testing uses uniform positions. L31-32 `temporal_sampling`:
  `torch.linspace(start_idx, end_idx, num_samples)`, clamped, then `.long()`.

### Kinetics clip length

- Kay et al. 2017, arXiv:1705.06950, abstract: "Each clip lasts around 10s and is taken from a
  different YouTube video." https://arxiv.org/abs/1705.06950 . Matches.

### Net for the claim

- "16 frames, stride 4" is exactly VideoMAE's K400 setting.
- "One clip per video" holds for **training** in both codebases.
- **Evaluation differs**:
  - VideoMAE uses 5 clips × 3 crops and, in test mode, decodes every 4th frame of the whole ~10 s video.
  - SlowFast uses 10 clips × 3 crops.
- SlowFast samples a 64-frame raw window, so its span equals 16×4. The Fast pathway, however, reads 32
  frames at stride 2.
- Training input is 224×224 crops. VideoMAE takes them from short-side-320 sources. SlowFast resizes
  the short side to a random value in [256, 320] first. Testing uses 224 (VideoMAE) or 256 (SlowFast).

---

## 2. Video-language models: "32 frames evenly spaced over the whole video (also 8 and 64)"

**Verdict: matches for the LLaVA family (whole-video uniform `np.linspace`). LLaVA-Video's default is
64 frames, not 32. LLaVA-OneVision uses up to 32. Differs for Qwen2-VL / Qwen2.5-VL: their frame count
follows the frame rate (2 fps, 4-768 frames, even count), so it is not a fixed 32.**

### LLaVA-Video-7B-Qwen2

- Model card https://huggingface.co/lmms-lab/LLaVA-Video-7B-Qwen2 (README.md). L136: "This model
  support at most 64 frames." `load_video` at L174-191:
  ```python
  fps = round(vr.get_avg_fps()/fps)
  frame_idx = [i for i in range(0, len(vr), fps)]
  if len(frame_idx) > max_frames_num or force_sample:
      uniform_sampled_frames = np.linspace(0, total_frame_num - 1, sample_fps, dtype=int)
  ```
  L199-200: `max_frames_num = 64`, called with `force_sample=True`.
- Formula: `idx_k = floor(k*(N-1)/(F-1))` for k = 0..F-1, with F = 64. `dtype=int` truncates.
- Training: LLaVA-NeXT `llava/utils.py` L25-38 `process_video_with_decord` uses the same logic
  (1-fps candidates, then `np.linspace(0, total_frame_num - 1, frames_upbound, dtype=int)` when
  `force_sample` is set or the 1-fps count exceeds `frames_upbound`). `llava/train/train.py`
  L131-134 defaults: `video_fps=1`, `frames_upbound=0`, `force_sample=False`. The LLaVA-Video recipe
  `scripts/video/train/SO400M_Qwen2_7B_ov_to_video_am9.sh` L79-82 passes `--frames_upbound 64`,
  `--add_time_instruction True`, `--force_sample True`.
- Paper (Zhang et al., arXiv:2410.02713), sec. on Table 8: "the first group shows an increase in the
  number of frames from 32 to 110. We set 110 frames as the upper limit". The Qwen2-72B note says
  "we could only process 8 frames" without pooling.

### LLaVA-OneVision

- Paper (Li et al., arXiv:2408.03326), Appendix C.1: "We sample up to 32 frames per video, leading to a
  maximum of 32 × 196 video tokens."
- LLaVA-NeXT `docs/LLaVA_OneVision_Tutorials.ipynb` (raw JSON lines 324-338): `load_video` uses
  `np.linspace(0, total_frame_num - 1, max_frames_num, dtype=int)`. The example call is
  `load_video(video_path, 16)`, so the inference demo uses 16 frames.

### Qwen2-VL / Qwen2.5-VL (qwen-vl-utils)

- Constants. `vision_process.py` at Qwen repo HEAD has L31-34; PyPI 0.0.8 has L32-35 and 0.0.11 has
  L33-36: `FPS = 2.0`, `FRAME_FACTOR = 2`, `FPS_MIN_FRAMES = 4`, `FPS_MAX_FRAMES = 768`.
  https://github.com/QwenLM/Qwen2.5-VL/blob/main/qwen-vl-utils/src/qwen_vl_utils/vision_process.py
  (now served from QwenLM/Qwen3-VL)
- `smart_nframes` (HEAD L144-181):
  - If `nframes` is given: `nframes = round_by_factor(nframes, 2)`.
  - Otherwise:
    - `min_frames = ceil_by_factor(min_frames or 4, 2)`
    - `max_frames = floor_by_factor(max_frames or min(768, total_frames), 2)`
    - `nframes = total_frames / video_fps * fps`, where fps defaults to 2.0
    - `nframes = min(min(max(nframes, min_frames), max_frames), total_frames)`
    - `nframes = floor_by_factor(nframes, 2)`
  - Validates `2 <= nframes <= total_frames`.
  - **Version note:** qwen-vl-utils 0.0.8 (2024-09-24, the Qwen2-VL era, L151-159) used
    `round_by_factor` for the last step, not floor, and had no `total_frames` cap. 0.0.11 (2025-04)
    and later use `floor_by_factor`. So "rounding to a multiple of 2" is accurate for 0.0.8. Current
    versions round down.
- Index selection: `idx = torch.linspace(start_frame, end_frame, nframes).round().long()`.
  - torchvision path L215 uses `0, total_frames - 1`. decord path L317 and torchcodec path L366 use
    `start_frame, end_frame`.
  - `calculate_video_frame_range` (HEAD ~L234-285) gives `start_frame = ceil(video_start*fps)` or 0 and
    `end_frame = floor(video_end*fps)` or `total_frames-1`.
  - **Formula:** `idx_k = round(s + k*(e-s)/(n-1))`, using round-half-to-even (`torch.round`), not
    truncation as in LLaVA.
- Backend choice (HEAD L386-399; 0.0.11+): env `FORCE_QWENVL_VIDEO_READER`, otherwise torchcodec if
  installed, then decord, then torchvision. The torchcodec path builds
  `VideoDecoder(video_path, num_ffmpeg_threads=TORCHCODEC_NUM_THREADS)` (default 8, L353-357) and calls
  `get_frames_at(indices=idx)` (L368). 0.0.8 had only decord and torchvision.
- Papers:
  - Qwen2-VL (arXiv:2409.12191): "we sampled each video at two frames per second"; evaluation "limited
    the maximum number of frames extracted per video to 768".
  - Qwen2.5-VL (arXiv:2502.13923): "dynamic FPS sampling ... we dynamically sampled FPS during
    training".

### Net for the claim

- "Evenly spaced over the whole video" is correct for both families.
- "32 frames" fits LLaVA-OneVision (cap 32).
- LLaVA-Video's card and recipe use **64**. Its paper ablates 32-110, and 8 is the unpooled 72B memory
  limit.
- Qwen's frame count **depends on duration** (2 fps): a 10 s clip gives 20 frames and a 60 s clip
  gives 120. It is clamped to [4, 768] and is even.
- Both index formulas start at frame 0 and end at frame N-1 (inclusive endpoints). LLaVA truncates and
  Qwen rounds.

---

## 3. Robotics (LeRobot): "1-4 frames by timestamp at random times; AV1 low resolution"

**Verdict: mostly matches. Frames are requested by timestamp, 1-4 per camera per item (most policies
use 1, diffusion uses 2, the doc example uses 4), and AV1 yuv420p at low resolution. Two corrections:
LeRobot does not call `get_frames_played_at`, and its torchcodec decoder uses `seek_mode="approximate"`.**

- Dataset defaults (`src/lerobot/datasets/lerobot_dataset.py`):
  - L61 `tolerance_s: float = 1e-4`, also L779 and L889.
  - L65 `video_backend: str | None = None`.
  - L195 doc: "Defaults to torchcodec when available ... otherwise, defaults to 'pyav'."
  - L248 `get_safe_default_video_backend()`.
  - Same 1e-4 default in lerobot 0.6.1 (`lerobot_dataset.py` L55, `streaming_dataset.py` L253).
- `src/lerobot/utils/import_utils.py` L91-103: returns `"torchcodec"` if importable, else `"pyav"`.
- delta_timestamps example, `examples/dataset/load_lerobot_dataset.py` L115-122:
  `camera_key: [-1, -0.5, -0.20, 0]` ("loads 4 images: 1 second before current frame, 500 ms before,
  200 ms before, and current frame"). `"observation.state": [-1.5, -1, -0.5, -0.20, -0.10, 0]`.
  `"action": [t / dataset.fps for t in range(64)]`.
- Policies:
  - `policies/diffusion/configuration_diffusion.py` L104 `n_obs_steps: int = 2`; L251-252
    `observation_delta_indices = list(range(1 - n_obs_steps, 1))`, giving `[-1, 0]` (two consecutive
    frames).
  - ACT, pi0, pi05, SmolVLA, TD-MPC, GR00T and others: `n_obs_steps = 1`. ACT and pi0
    `observation_delta_indices` return `None`, so only the current frame is used.
  - VQ-BeT: `n_obs_steps = 5`. That exceeds the claimed 1-4 range (0.6.1 configs).
- How frames are queried:
  - `src/lerobot/datasets/dataset_reader.py` L429-450 `_query_videos` shifts the episode-relative
    timestamps by `videos/{key}/from_timestamp`. In v3.0, many episodes are concatenated into one MP4
    file per chunk (`utils.py` L163-165 `shift_timestamps`). It then calls `decode_video_frames`.
  - `src/lerobot/datasets/video_utils.py`:
    - L121-170 `decode_video_frames` dispatches to torchcodec or pyav.
    - L322-410 `VideoDecoderCache` is an LRU cache of `VideoDecoder` objects, default capacity
      `DEFAULT_DECODER_CACHE_SIZE = 100` (L292, env `LEROBOT_VIDEO_DECODER_CACHE_SIZE`). It opens the
      file through fsspec and constructs **`VideoDecoder(file_handle, seek_mode="approximate")`**
      (L375).
    - L415-477 `decode_video_frames_torchcodec`:
      ```python
      frame_indices = [round(ts * average_fps) for ts in timestamps]   # L450
      frames_batch = decoder.get_frames_at(indices=frame_indices)       # L454
      ```
      It then checks `|query_ts - frames_batch.pts_seconds| <= tolerance_s` in float64 and raises
      `FrameTimestampError` if the check fails (L457-470).
    - lerobot 0.6.1 does the same (`video_utils.py` L309 `seek_mode="approximate"`, L387-389
      `round(ts*average_fps)` then `get_frames_at`).
  - **So LeRobot does not use `get_frames_played_at`.** It converts timestamps to indices with
    `average_fps` and uses index access in approximate mode.
- "At random times": training uses a shuffled sampler over all frame rows. Each item is one timestep,
  plus its delta window, inside one episode. Seen per file, this gives random seeks into a long
  concatenated MP4. (Inferred from the dataset design. I did not cite a specific sampler line.)
- Encoding defaults (`src/lerobot/configs/video.py`, identical in 0.6.1):
  - L89 `vcodec: str = "libsvtav1"`, L90 `pix_fmt: str = "yuv420p"`, L91 `g: int | None = 2`
    ("GOP size (keyframe interval)"), L92 `crf: ... = 30`.
  - L47 `LIBSVTAV1_DEFAULT_PRESET: int = 12`, applied when `preset is None` (L109-110).
  - L45 alias `"av1" -> "libsvtav1"`. `"auto"` picks a hardware encoder if one is available.
  - `encode_video_frames` (`video_utils.py` L480+) uses these fields through PyAV `add_stream(vcodec,
    fps, options=...)`.
- Typical resolutions, from `meta/info.json` on the Hub (all `video.codec: av1`, `yuv420p`, v3.0):

  | Dataset | Camera shape (H, W) | fps |
  |---|---|---|
  | `lerobot/pusht` | 96×96 | 10 |
  | `lerobot/libero` | 256×256 (2 cams) | 10 |
  | `lerobot/droid_1.0.1` | 180×320 (3 cams) | 15 |
  | `lerobot/aloha_sim_insertion_human` | 480×640 | 50 |
  | `lerobot/aloha_static_coffee` | 480×640 (4 cams) | 50 |
  | `lerobot/svla_so101_pickplace` | 480×640 (2 cams) | 30 |

  URL pattern: `https://huggingface.co/datasets/<repo>/raw/main/meta/info.json`.
- Net: "AV1 low resolution" holds. The usual sizes are 96² to 640×480 with GOP 2 (a keyframe every 2
  frames), so a random access decodes at most 1-2 frames.

---

## 4. Video generation control case: "64 consecutive frames from a random start"

**Verdict: within range on frame count. Random start plus a contiguous window is the standard pattern.
Exact stride 1 is common but not universal: Open-Sora downsamples to 24 fps or less (stride 2 for
30/60 fps sources), Latte uses stride 3, and CogVideoX fine-tuning spans the whole clip from frame 0
with no random start.**

- **Open-Sora 2.0** (hpcaitech/Open-Sora `main`):
  - `configs/diffusion/train/image.py` L2-7: `fps_max=24` ("the desired fps for training").
  - `configs/diffusion/train/stage1.py` L7-42 and `stage2.py` L17-51: `bucket_config` frame counts
    1, 5, 9, ..., 65, ..., 129 (4k+1) at 256px and 768px.
  - `opensora/datasets/utils.py` L294-313 `map_target_fps`: `sampling_interval = ceil(fps/max_fps)`
    if `fps >= 24`, else 1. A 30 fps source gives stride 2. A 24 fps source gives stride 1.
  - L85-96 `temporal_random_crop`: `TemporalRandomCrop(num_frames*frame_interval)` picks a random
    window, then `np.linspace(start, end-1, num_frames, dtype=int)`.
  - `opensora/datasets/datasets.py` L198-211: optional `rand_sample_interval` draws a random stride in
    [1, k]. The video is read in full with `read_video(path, backend="av")` (L202).
  - 64 or 65 frames at stride 1-2 from a random start is squarely within range.
- **CogVideoX** (zai-org/CogVideo):
  - Paper arXiv:2408.06072, abstract: "10-second continuous videos ... with a frame rate of 16 fps".
    Sec. 3D VAE: "Randomly select 8 or 16 fps".
  - Fine-tuning `finetune/README.md` L76: frames "must be a multiple of 8 plus 1 (i.e., 8N+1), such as
    49, 81"; L138-140 recommends a maximum of 49. `finetune/train_ddp_t2v.sh` L25:
    `--train_resolution "81x768x1360"`.
  - `finetune/datasets/utils.py` L142-144:
    `indices = list(range(0, video_num_frames, video_num_frames // max_num_frames))`, truncated to
    `max_num_frames`. So the start is fixed at 0 and the stride is `N // F`. It is not random-start
    contiguous.
- **Latte** (Vchitect/Latte): `configs/ffs/ffs_train.yaml` and `configs/ucf101/ucf101_train.yaml`
  L13-16: `num_frames: 16`, `image_size: 256`, `frame_interval: 3`. The datasets use
  `TemporalRandomCrop` plus `np.linspace` (`datasets/ffs_datasets.py` ~L153).
- Inference lengths, for scale: Wan2.1 `generate.py` L83-85 defaults to `frame_num = 81`. The
  HunyuanVideo README (L229-321) lists 129-frame configs.
- Net: the common lengths are 49/65/81/129 (4k+1 or 8k+1, because of causal-VAE temporal compression).
  64 is close but not that shape. If the benchmark wants realism, 65 at stride 1 (24 fps source) or
  stride 2 (30-60 fps source) matches Open-Sora exactly.

---

## 5. torchcodec 0.17 public API (docs and release notes only)

Sources:
- https://meta-pytorch.org/torchcodec/0.17/generated/torchcodec.decoders.VideoDecoder.html
- https://meta-pytorch.org/torchcodec/0.17/generated/torchcodec.decoders.set_cuda_backend.html
- https://meta-pytorch.org/torchcodec/0.17/generated_examples/decoding/custom_frame_mappings.html
- https://meta-pytorch.org/torchcodec/0.17/generated_examples/decoding/basic_cuda_example.html
- https://meta-pytorch.org/torchcodec/0.17/generated/torchcodec.decoders.set_nvdec_cache_capacity.html
- GitHub releases v0.11.0, v0.12.0, v0.17.0

Version: PyPI `torchcodec` latest is 0.17.0 (2026-09-30), compatible with `torch >= 2.11`, Python >= 3.10.

- **Signature** (VideoDecoder page):
  ```
  VideoDecoder(source, *, stream_index=None, dimension_order='NCHW', num_ffmpeg_threads=1,
  device=None, seek_mode='exact', transforms=None, output_dtype=torch.uint8,
  custom_frame_mappings=None)
  ```
- **seek_mode**: "Exact guarantees that requesting frame i will always return frame i, but doing so
  requires an initial scan of the file. Approximate is faster as it avoids scanning the file, but less
  accurate as it uses the file's metadata to calculate where i probably is. Default: 'exact'." The
  approximate-mode tutorial adds: "The scan doesn't involve decoding, but processes an entire file".
- **custom_frame_mappings**: "Mapping of frames to their metadata, typically generated via ffprobe.
  This enables accurate frame seeking without requiring a full video scan. Do not set seek_mode when
  custom_frame_mappings is provided."
  - Expected JSON: `{"frames": [{"pts": 0, "duration": 1001, "key_frame": 1}]}`.
  - "Alternative field names 'pkt_pts' and 'pkt_duration' are also supported."
  - Generation (custom_frame_mappings tutorial, "Creating custom frame mappings with ffprobe"):
    ```
    ffprobe -i <video> -select_streams <stream_index> -show_frames -show_entries frame=pts,duration,key_frame -of json
    ```
  - The tutorial says the mapping "speed[s] up VideoDecoder instantiation, while maintaining the frame
    seeking accuracy of seek_mode='exact'". Note that `-show_frames` decodes the file once, so the
    cost moves to preprocessing.
- **num_ffmpeg_threads**: "The number of threads to use for CPU decoding. This has no effect when using
  GPU decoding ... Passing 0 lets FFmpeg decide ... Default: 1."
- **device**: "If None (default), uses the current default device. If you pass a CUDA device, decoding
  uses the 'nvdec' backend by default. See set_cuda_backend() to switch to the 'ffmpeg' CUDA backend."
- **set_cuda_backend(backend: str)**, a context manager: "The default is 'nvdec'. An 'ffmpeg' backend is
  also available. Only the creation of the decoder needs to be inside the context manager ... This is
  thread-safe and async-safe." Parameter: `"nvdec"` (default) or `"ffmpeg"`.
  - **The 0.17 docs list only `"nvdec"` and `"ffmpeg"`. They do not list `"beta"`.**
  - History: `"beta"` was introduced in 0.8 (v0.8.1 notes). v0.11.0 notes say "in the next release, the
    'beta' backend will become the default". v0.12.0 notes: "the faster CUDA backend (previously known
    as 'beta') becomes the default backend". The docs for 0.12 through 0.17 all show the `nvdec` /
    `ffmpeg` wording.
  - Whether `"beta"` is still accepted as an alias in 0.17 is not documented. I did not check it,
    since source reading is out of scope and torchcodec is not installed here. Use `"nvdec"`.
- **get_frames_at(indices: Tensor | list[int]) -> FrameBatch**: "Return frames at the given indices."
  **get_frames_played_at(seconds: Tensor | list[float]) -> FrameBatch**: "Return frames played at the
  given timestamps in seconds." Also available: `get_frames_in_range(start, stop, step=1)`,
  `get_frames_played_in_range(start_seconds, stop_seconds, fps=None)`, `get_all_frames(fps=None)`,
  `get_frame_at`, `get_frame_played_at`, and `__getitem__`. The docs recommend batch methods.
- **cpu_fallback** (variable, `CpuFallbackStatus`): "Use bool(cpu_fallback) to check if fallback
  occurred, or str(cpu_fallback) to get a human-readable status message. The status is only determined
  after at least one frame has been decoded." The CUDA tutorial says fallback "can happen when the video
  codec or format is not supported by the NVDEC hardware decoder, or when NVCUVID wasn't found".
  Example output: `[CUDA] Fallback status: No fallback required`.
- **AV1 on the NVDEC backend**: supported.
  - v0.17.0 notes, Improvements: "HEVC 4:4:4 (8, 10 and 12-bit) and 10-bit AV1 now decode natively on
    the GPU instead of falling back to the CPU". This implies 8-bit AV1 was already native
    before 0.17, and that 10-bit AV1 fell back to CPU before 0.17.
  - v0.2.0 notes: "Fixed AV1 decoding on CUDA".
  - Hardware support for AV1 also depends on the GPU generation, per the NVIDIA support matrix the
    tutorial links to. I did not verify which generations here.
- **Decoder caching**: yes.
  - `set_nvdec_cache_capacity(capacity)` page: "The NVDEC decoder cache stores hardware decoders for
    reuse ... The default capacity is 20 decoders per device ... evicted using a least-recently-used
    policy. Generally, a decoder can be re-used from the cache if it matches the same codec and frame
    dimensions." Setting 0 disables it. `get_nvdec_cache_capacity()` reads it.
  - v0.11.0 notes added multi-instance caching, LRU, and the capacity API.
  - v0.17.0 notes: "Fixed an NVDEC cache collision bug".
- **Also new in 0.17** (Beta): low-level `Demuxer` -> `VideoPacketDecoder` -> `ColorConverter` APIs with
  raw YUV planes, multi-stream decoding, and `video_stream.scan()`, which returns a `FrameIndex` with
  exact timestamps and key frames. The notes say "`VideoDecoder` and `AudioDecoder` are unchanged".

---

## Contradictions and corrections to the claims

1. **SlowFast is not "16 frames at stride 4".** The 8x8 config gives Slow 8 frames at stride 8, the
   4x16 config gives Slow 4 frames at stride 16, and Fast takes 32 frames at stride 2 in both. All of
   them read a 64-frame raw window (`NUM_FRAMES: 32`, `SAMPLING_RATE: 2`). Only VideoMAE is exactly
   16×4.
2. **VideoMAE fine-tuning spacing is not exactly 4.** It uses `np.linspace` over a 64-frame window,
   which gives about 4.27. Pretraining is exactly 4.
3. **Evaluation uses more than one clip per video.** VideoMAE uses 5 clips × 3 crops and, in test mode,
   decodes every 4th frame of the whole video. SlowFast uses 10 × 3.
4. **LLaVA-Video defaults to 64 frames, not 32.** OneVision uses up to 32, and its tutorial uses 16.
5. **Qwen2-VL / 2.5-VL do not use a fixed count.**
   - They sample at 2 fps, clamped to [4, 768] and even.
   - The last step rounds to even in 0.0.8 and floors to even in 0.0.11 and later.
   - Indices come from `torch.linspace(...).round()` (round-half-to-even). LLaVA uses
     `np.linspace(..., dtype=int)` (truncation).
6. **LeRobot does not call `get_frames_played_at`.** It computes `round(ts*average_fps)` and calls
   `get_frames_at`. Its cached `VideoDecoder` uses `seek_mode="approximate"`, then checks
   `pts_seconds` against `tolerance_s=1e-4`. VQ-BeT uses 5 observation frames, more than 1-4. v3.0
   datasets concatenate episodes, so timestamps carry an offset.
7. **Video generation lengths are usually 4k+1 or 8k+1** (49/65/81/129), not 64. Strides are 1-3.
   CogVideoX fine-tuning takes the whole clip from frame 0 at stride `N//F`, not a random start.
8. **The torchcodec 0.17 CUDA backend names are `"nvdec"` (default) and `"ffmpeg"`.** `"beta"` is the
   pre-0.12 name and is not documented in 0.17.
