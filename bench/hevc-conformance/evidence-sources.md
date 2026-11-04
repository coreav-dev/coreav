# HEVC G1 evidence sources

Evidence-producing commit: `028ca00a6831b64faa226614252edd0290ce642d`. The complete census accounts for 147 official archives and 144 distinct official bitstreams, including two explicit maintainer exclusions and one named scope refusal. Four synthetic Main10 sources are separate from that denominator.

Registration SHA-256: `8adb0eebffe919e8619af207a16dd5641afeba900cfb413f8f38e2bd2dd2bfa0`. Parser executable SHA-256: `af06884c3ddcda043e4b1b0af766288f1da1868178e4cb8a1dc993180e0741c6`. Known-exclusion registry SHA-256: `c1adb93a4915357a1639da928701e1e33440fe2c376c0f4f014e50e33295de14`.

Count methods: archived_yuv_length: 15; expected_refusal: 1; h265_1_prose_count: 2; hash_validated_oracle_count: 126; maintainer_exclusion: 2; published_output_count: 1; synthetic_registered_recipe: 4. Hash-validated oracle counts are not published counts. Exclusions and refusals receive no reference-count credit.

126 archive-linked rows, representing 123 distinct official streams, rely on a hash-validated oracle count. The table retains duplicate archive identities.

VPSSPSPPS_A retains the published description count of six. Its oracle-only preparation preserves every NAL byte and within-type order, while CoreAV reads the original. H.265.1 section 6.6.12.5 tests arbitrary parameter-set order, which the pinned oracle does not handle on the original: Captain decision, 2026-11-04. Its prepared SHA-256 is `d2097afe2c90bda74915fae48d567ac260b7932781644ba9cdb3ebbc69d315dd`; prepared output exactly matches the published MD5.

The two [known exclusions](known-exclusions.json) retain their published and observed MD5 disagreements and exhausted investigation. Synthetic hashes come from registered raw references and matching component SEI, not published H.265.1 pictures. These are header and future intra-candidate receipts, not a reconstruction verdict.

| Test | Stream SHA-256 | Result | Count method | Output count | Source | Reference / disposition |
|---|---|---|---|---:|---|---|
| AMP_A | 09a9c0d78a502ece45f1df02c773b18bb3133fe21a8f1e9046db19bfa68f7c29 | headers_checked | hash_validated_oracle_count | 17 | AMP_A_Samsung_7.md5 | 93a5875d58072db5539c04e1e943ed9d |
| AMP_B | 26112eeb6f10ad18ffd30334e62bf9dafdba6d4efb07ffe3c5b75562e4639028 | headers_checked | hash_validated_oracle_count | 17 | AMP_B_Samsung_7.md5 | 4e051fad4145d5127e16ac2999500069 |
| AMP_D | a0d7fea2136ae47739bab1fa8c760705538a20b35467f34606f4bf9da5019a63 | headers_checked | hash_validated_oracle_count | 100 | AMP_D_Hisilicon_3.yuv.md5 | 277006085088cf68b9cafbbba0c19443 |
| AMP_E | 2088d96297504a1e56da30a530d321fd1380ae48aeac7357d925da7b09f850dd | headers_checked | hash_validated_oracle_count | 100 | AMP_E_Hisilicon_3.yuv.md5 | 1a301e997917128420e3c33e6c14693a |
| AMP_F | 5976368a7f37557cdbc6d8313bf16419a7b533bef9b73e8afced86c23be9c208 | headers_checked | hash_validated_oracle_count | 100 | AMP_F_Hisilicon_3.yuv.md5 | 6be6b5fb49a9514914f4ea812f737076 |
| AMVP_A | dcbc3c321e66f8d4b06481ffbdb2e02214d83f60db5868823938715cfd275371 | headers_checked | hash_validated_oracle_count | 30 | AMVP_A_MTK_4/AMVP_A_MTK_4.md5 | 5dd1b1099391ad659225fbf94965a6c7 |
| AMVP_B | 6f863212547479c25f8f7d85b25d5ca6a1528cbc9e0ee6ff3fad799122d6c632 | headers_checked | hash_validated_oracle_count | 41 | AMVP_B_MTK_4/AMVP_B_MTK_4.md5 | 7fecb4fe8ee4d4838f4a9873b062b819 |
| AMVP_C | 5c51d71c974482ac9dfacef462936cac00ab9f3a0f34bb93fc8328a09e4fb21a | headers_checked | hash_validated_oracle_count | 60 | AMVP_C_Samsung_7.md5 | 75155ad034919a2846bbdcba13b5f733 |
| BUMPING_A | 817ba50355043ac04abd76898e0d7d713c7055fee4524f22f24ddae6500577d9 | headers_checked | h265_1_prose_count | 49 | H.265.1 6.6.12.33: POC 0..65, excluding 17 named POCs | 64f213eb1cf45614f0eeb50fd1b725bf |
| CAINIT_A | e7a4b9ab27dde802001582e8a189aaf8a72e7a95480b22c002a725fa02d25105 | headers_checked | hash_validated_oracle_count | 50 | CAINIT_A_SHARP_4.md5 | 550c5a918fecda6a284aa5f44c0e2f33 |
| CAINIT_B | d7f9bef49ab3c32bd6c53124f9c8f5e853b6706d6f32d2551046dea7ca2f4746 | headers_checked | hash_validated_oracle_count | 50 | CAINIT_B_SHARP_4.md5 | 0c633f8ee5a4c5779cff48ca86273685 |
| CAINIT_C | bfdf239164cef54252ead77cd76cd436b96847b1a0b7ccba547fbb601e4a34f1 | headers_checked | hash_validated_oracle_count | 50 | CAINIT_C_SHARP_3.md5 | 9c6c5363724a8bad54e2869b655fbf3b |
| CAINIT_D | 7bb128ed1ca68961548fe8bc749d66665651387d4cf2a45c6fdcdc713dd4f5b8 | headers_checked | hash_validated_oracle_count | 50 | CAINIT_D_SHARP_3.md5 | 0231e327044e45971fc30a43a1bd8275 |
| CAINIT_E | ef55f131f79e2f7f376131a68118ff41d89e7bdf9a02fc7e5b5aeb85563a3dfa | headers_checked | hash_validated_oracle_count | 50 | CAINIT_E_SHARP_3.md5 | 55af76d5527af999d12ccc37b1f6b7ae |
| CAINIT_F | eac6cebd384d31fbcea7983b17bb76290d77f865a3932ff1f05a4412cbcf9cfc | headers_checked | hash_validated_oracle_count | 50 | CAINIT_F_SHARP_3.md5 | 88d8cd2d303b662f0a92a25256957b69 |
| CAINIT_G | caf40ca73dfea0efe2a8f7010036b25176d85d6959b12b5456fdaab44fd2d61d | headers_checked | hash_validated_oracle_count | 50 | CAINIT_G_SHARP_3.md5 | a7036eac7798f6e8691f0975786d6811 |
| CAINIT_H | c658a14f3ae68bb137c37989d1789a2b09e0f313ea8739a02966207f7d64c0c9 | headers_checked | hash_validated_oracle_count | 50 | CAINIT_H_SHARP_3.md5 | cfdd3ce0f4cf1eafd012dcb266311d06 |
| CIP_A | 19f6a710e8b2c66f393392af24c11e5fbba10a5c9f6f28c8b26949942f384c1c | headers_checked | hash_validated_oracle_count | 2 | CIP_A_Panasonic_3/CIP_A_Panasonic_3_yuv.md5 | 4cbf601ba98d63f642defab5eaa12c8d |
| CIP_B | 786e5bd6ac5418f1e4950fc9c5251826128b15d94eda29679b72a4f79d8a77f7 | headers_checked | hash_validated_oracle_count | 5 | cip_B_NEC_3/cip_B_NEC_3.md5 | 1315714c7b3f3237b469259f66d9b7aa |
| CIP_C | 1a696ecc7520b23c89ac6daca7ab1f1f325a01dc01845ba352528be775456ed5 | headers_checked | hash_validated_oracle_count | 2 | CIP_C_Panasonic_2/CIP_C_Panasonic_2_yuv.md5 | ed985932fef2aa8a74ded970070bdab2 |
| CONFWIN_A | 688c2684539ffdca60335e7da650da0236f76c20179c844a33395cceaebb1ca3 | headers_checked | hash_validated_oracle_count | 60 | CONFWIN_A_Sony_1/CONFWIN_A_Sony_1.md5 | a3ce3f936ff69ff1ec2621a622dd37ac |
| DBLK_A_MAIN10 | 3a9089207bedddb0c8db28b410441e37d4cff5dd8301cb8ba7d2f75036e2d50c | headers_checked | hash_validated_oracle_count | 8 | DBLK_A_MAIN10_VIXS_4/DBLK_A_MAIN10_VIXS_4.md5 | c4594956bb9e8303f1662f9eb1bcdf50 |
| DBLK_A | d61a13e04a1678816c0f16cf7a47f5bb918de95ecf1b6fab305e2390c3dd7de9 | headers_checked | hash_validated_oracle_count | 30 | DBLK_A_SONY_3.bit.yuv.md5 | 42486a48ea12d5ab6cd98ed2e2807cfe |
| DBLK_B | 1800b72fdf655d1ad399ff4d35fb2789c2b89640c95045e3c1502c74efa232b0 | headers_checked | hash_validated_oracle_count | 30 | DBLK_B_SONY_3.bit.yuv.md5 | cf5de25a94a33e2d4237b8d1c773db06 |
| DBLK_C | 039835a155f8e72c5f3876ec63d0667e06928959d2767dd1764f1148d2d8c2e2 | headers_checked | hash_validated_oracle_count | 30 | DBLK_C_SONY_3.bit.yuv.md5 | 41f89edc63b1175ffc1fae909f4fed82 |
| DBLK_D | 7ac7196d25d339ba2be54b50d184463d47b8ba546cbb8c0a2a8882043bda1056 | headers_checked | hash_validated_oracle_count | 8 | DBLK_D_VIXS_2/DBLK_D_VIXS_2_yuv.md5 | f6f3075edca573f5dfe4e352d26170f1 |
| DBLK_E | 1e48dc12c9107f28463772b5ea1de31201181b0c4b66b11f66f4aa13ac26ce74 | headers_checked | hash_validated_oracle_count | 8 | DBLK_E_VIXS_2/DBLK_E_VIXS_2_yuv.md5 | 1f94d56d9e462967cad73284ed0f43f9 |
| DBLK_F | 726c5fe34048cb3e19f69d28f9bec067bc632b13637e452fb0a536a1e32d93dd | headers_checked | hash_validated_oracle_count | 8 | DBLK_F_VIXS_2/DBLK_F_VIXS_2_yuv.md5 | c94d0c0d80c93a2edd372a1fd12f93a1 |
| DBLK_G | 1e48dc12c9107f28463772b5ea1de31201181b0c4b66b11f66f4aa13ac26ce74 | headers_checked | hash_validated_oracle_count | 8 | DBLK_G_VIXS_2/DBLK_G_VIXS_2_yuv.md5 | 1f94d56d9e462967cad73284ed0f43f9 |
| DELTAQP_A | 1274e3e308dfe684b38a88f8b3a2845b84f57492b0b02aaecbe5dc29ca8ed532 | headers_checked | hash_validated_oracle_count | 96 | DELTAQP_A_BRCM_4/DELTAQP_A_BRCM_4_yuv.md5 | 1ceef83183197fd1f21472c17b53cbff |
| DELTAQP_B | 07b8fa589e71be769874d95a480e6c253ce8dfc730dc05ec1ad24c444752e5ee | headers_checked | hash_validated_oracle_count | 30 | DELTAQP_B_SONY_3.bit.yuv.md5 | f02fa96633c63bd5e1ce0f103447d1ed |
| DELTAQP_C | 4fc85c7ba2be608b3aa5ec3be4cf4e9c21c390ca48335b4c99e6fe4e429e8fe3 | headers_checked | hash_validated_oracle_count | 30 | DELTAQP_C_SONY_3.bit.yuv.md5 | 1c2be685b1ae953787e0e93c656ee502 |
| DSLICE_A | 8398fb23c814a197bba497ad2c6103f81ca8003434fa40ec347d4d0a07c9468a | headers_checked | hash_validated_oracle_count | 50 | DSLICE_A_HHI_5/DSLICE_A_HHI_5.md5 | c7caf3164b0a316549ac7244f66f1294 |
| DSLICE_B | db582f1181f3b8ed3beeb32246a1a99123700dbdc3d3be4e1797032fcd46c486 | headers_checked | hash_validated_oracle_count | 50 | DSLICE_B_HHI_5/DSLICE_B_HHI_5.md5 | 2e072130f116a4d0f7fc69fa76cfcdfb |
| DSLICE_C | f162007f604aa0e48229c4153f940fd5f24db955ad158d11e2bd46c89c49ca76 | headers_checked | hash_validated_oracle_count | 50 | DSLICE_C_HHI_5/DSLICE_C_HHI_5.md5 | 505a47c5ce4bc66ef5661aeb778b0f1f |
| ENTP_A | 1dc1dfb16f676f59cebde3a4732209ce5ce0d0f49ef697970987c0c3e0e087b1 | headers_checked | hash_validated_oracle_count | 24 | ENTP_A_Qualcomm_1.md5 | 25f3c7facf9e7b75e81247206b4fc60d |
| ENTP_B | 8f7320cc17416a1a4b53d01b94b031edd01c804affad3474b58d910a23af3bd4 | headers_checked | hash_validated_oracle_count | 24 | ENTP_B_Qualcomm_1.md5 | 6577f102b93577eb899e0bff1a20a401 |
| ENTP_C | 993cd68f0e749cd1d9ce7cc3a368b4df26dccf07ded91048100217b9a0c56d28 | headers_checked | hash_validated_oracle_count | 24 | ENTP_C_Qualcomm_1.md5 | 0b607e6e7946e6e8d0f92a1a294d135d |
| EXT_A | 1322c36a7c283f258fed90fc15f6f8dfe40bd621b090716dde9cce5c06f71e05 | headers_checked | h265_1_prose_count | 3 | H.265.1 6.6.12.36 | 5797b3114fe12daf1ce522ed70ce4078 |
| FILLER_A | 63669795a10c50c898ef19bd75568cd8a6ae0a16797898f629692d4c6315fa31 | headers_checked | hash_validated_oracle_count | 60 | FILLER_A_Sony_1/FILLER_A_Sony_1.md5 | f92a928770a41b63e7cfc89ea198790f |
| HRD_A | 61af83565147e9dc23842847111e748d32d95830d88cd3855a66143ea08d6915 | headers_checked | hash_validated_oracle_count | 96 | HRD_A_Fujitsu_3.md5 | f6d04dba2ef09bcadbea7b8ab5c8c917 |
| INITQP_A | 24bdf9511d93696aff09568f8698edae81fff52a6b551fc7d79965f8b4e10cab | headers_checked | hash_validated_oracle_count | 60 | INITQP_A_Sony_1/INITQP_A_Sony_1.md5 | 2716af552e71ef61221b86130ad3d444 |
| INITQP_B_Main10 | 47a3eeabb3896b0af8f60c7a32475d69047409d609a740ef8b65c50c36c01401 | headers_checked | hash_validated_oracle_count | 90 | INITQP_B_Main10_Sony_1/INITQP_B_Main10_Sony_1.md5 | f63c71e5f6b9c6ddf2b080524f9f14b1 |
| IPCM_A | f345fa066789908f6d2995a9d4fc7cee48a466c705857c75378e1d650feae30b | headers_checked | hash_validated_oracle_count | 1 | ipcm_A_NEC_3/ipcm_A_NEC_3.md5 | 8049988c383486e076ea2494edda3831 |
| IPCM_B | cf1a824624d1cc26aca3d12656e30716b039e5edb48e41c3dd1c74538157f428 | headers_checked | hash_validated_oracle_count | 1 | ipcm_B_NEC_3/ipcm_B_NEC_3.md5 | 23a3b7024fd9bc64b946b9961ab0f51e |
| IPCM_C | a9800f093e66e7cb3147323e6ce6adc47620dfa4bbb89f15afb7f82b1e426977 | headers_checked | hash_validated_oracle_count | 1 | ipcm_C_NEC_3/ipcm_C_NEC_3.md5 | c3e74c399b73a5ab2dbd20523f583464 |
| IPCM_D | acb72389ebab10574a4821fd0a3329a307f3cf384a9ef67e6d0e761e807d8689 | headers_checked | hash_validated_oracle_count | 1 | ipcm_D_NEC_3/ipcm_D_NEC_3.md5 | aa64a16240064bc2a90fadf979a62a7b |
| IPCM_E | 1c0652e89cffbfd45625371cbaca3b2293b09593672c9d81b63b83c6a5b55416 | headers_checked | hash_validated_oracle_count | 1 | ipcm_E_NEC_2/ipcm_E_NEC_2.md5 | e1cd7a16d3f6a342855044ccba3e41f5 |
| IPRED_A | db440d8ce43ba3706a42b7c42095464dace247dbc4651d37c9787e5eb4aa3875 | headers_checked | hash_validated_oracle_count | 20 | IPRED_A_docomo_2.md5 | 0e4e9bd00b74e30e400de46ee7ad8023 |
| IPRED_B | 456608ae5c14fea0496d770ae9614cd3d7c6ae98c14b22d64bbd96b37f49d7a0 | headers_checked | archived_yuv_length | 1 | IPRED_B_Nokia_3.yuv | ec3bcad4f9174404e52d132206b6617b |
| IPRED_C | dbad5ee9c095c0336deb4ab7c7785153da20f10ef941cac0fd27c04cb9c9ec95 | headers_checked | hash_validated_oracle_count | 4 | IPRED_C_Mitsubishi_3/IPRED_C_Mitsubishi_3_yuv.md5 | 6043c04fecc60ee88f4247aaca96441f |
| LS_A | 14251238f005a4576437429a327b34d216b346aaac2a7a05c132a3430174b11b | headers_checked | hash_validated_oracle_count | 9 | LS_A_Orange_2/LS_A_Orange_2_yuv.md5 | e14f82958f87a87be73a6f51e76ffdab |
| LS_B | 0d9a71541f971d7694dda1c829377cf1a30c6c55f8e2c39fecb6b24ff9e82519 | headers_checked | hash_validated_oracle_count | 25 | LS_B_Orange_4/LS_B_Orange_4_yuv.md5 | 05ac9b36977c98001cc6c5a45e363662 |
| LTRPSPS | 8f8e50fa408b7e96e76e6936b380fc34c208271efb8c283b15a42cb1f8c78ae7 | headers_checked | hash_validated_oracle_count | 500 | LTRPSPS_A_Qualcomm_1.md5 | e925d467c20180090c09c02d02c3aa25 |
| MAXBINS_A | efddf49ed997d5a0ddcc7c85fe8f609768d4e8ddd88b32532e0833144ec939e4 | headers_checked | hash_validated_oracle_count | 2 | MAXBINS_A_TI_5/MAXBINS_A_TI_5_yuv.md5 | 1386fb149ddc11bd5a1d9111879acfa4 |
| MAXBINS_B | 783ced32f7b465a4701731bcaae72687133fd49a4a6488660ce8d6e5cf1d0259 | headers_checked | hash_validated_oracle_count | 3 | MAXBINS_B_TI_5/MAXBINS_B_TI_5_yuv.md5 | 21d6910760ea4963eab28b26709471fe |
| MAXBINS_C | 8c232aafbfa9366e9e677d698649a670237fde21a94c32fc9bc7b896fec70b91 | headers_checked | hash_validated_oracle_count | 3 | MAXBINS_C_TI_5/MAXBINS_C_TI_5_yuv.md5 | 7a7e51fdd40c57d70af418f5082416fa |
| MERGE_A | 0a84103e548cf8944851cc33c7826ea5213245fd94517fd1071895740abce749 | headers_checked | hash_validated_oracle_count | 8 | MERGE_A_TI_3/MERGE_A_TI_3.md5 | 3a15886a0158ef1aaf557869b2e9c522 |
| MERGE_B | a61399c23cff77098300cf96392cbcedd70c82c474458f8acf67a515df08cf19 | headers_checked | hash_validated_oracle_count | 8 | MERGE_B_TI_3/MERGE_B_TI_3.md5 | 83b4dc9f229a8fba3777d9f4f87ce144 |
| MERGE_C | 42096904b2145ed70a556aba2921f5e97284c8330f9b2aee50a7a1eae589cb51 | headers_checked | hash_validated_oracle_count | 8 | MERGE_C_TI_3/MERGE_C_TI_3.md5 | 216f0cc869e3989c8fb2cf261243160b |
| MERGE_D | 60a43e5ca40baa0e77617b9b392a2b6c75950ca0a9f8314ac42b01aea1f9880d | headers_checked | hash_validated_oracle_count | 8 | MERGE_D_TI_3/MERGE_D_TI_3.md5 | 104cccf2b8441862624777955d6ecdc4 |
| MERGE_E | e5f1f706176e91b72d13686b8864fe1c8e8be764faa417e3cabbf0df1cf78af0 | headers_checked | hash_validated_oracle_count | 8 | MERGE_E_TI_3/MERGE_E_TI_3.md5 | 6e74dbd450123246c22bf01b389bbe11 |
| MERGE_F | bdf3cdaf93284dfdabb4217bfa0e11ca2a54cd618e4d3b94ec131a1c73eabd36 | headers_checked | hash_validated_oracle_count | 41 | MERGE_F_MTK_4/MERGE_F_MTK_4.md5 | a8f0982f026dfda1c02b4d4d3eb88132 |
| MERGE_G | a7b157215595b68ddbb70d952fb786923826c26ecc448bbfe36f4df47f9abc5c | headers_checked | hash_validated_oracle_count | 2 | MERGE_G_HHI_4/MERGE_G_HHI_4.md5 | c82f4c88d9b2c73fabbeac8944da35a1 |
| MVCLIP_A | 9b287e76e0aa6943239debb517250e26489e72ad1bf504b5b2dd77d2a57dcba5 | headers_checked | hash_validated_oracle_count | 5 | MVCLIP_A_qualcomm_3/MVCLIP_A_qualcomm_3.yuv.md5 | 78b398c201089b6b17bcbda2bbccf3ae |
| MVDL1ZERO_A | 872a2ee8016796ffec9cf9d76fc91ce3a062ffb26a3d8468eac202367c2230ad | headers_checked | hash_validated_oracle_count | 500 | MVDL1ZERO_A_docomo_4.md5 | 593d1936f6fc2e1775ea2a691a601c34 |
| MVEDGE_A | aa19456c21b304b23e7dc568e8546982d66547bba15225a5d13b65ea27befa10 | headers_checked | hash_validated_oracle_count | 17 | MVEDGE_A_qualcomm_3/MVEDGE_A_qualcomm_3.yuv.md5 | 101c5bfe16ff1739909161794af29a2d |
| NoOutPrior_A | cd01eedf3929a1ca4d917e933dfa5011ce7a5cc6ea46b23cb78a6e638fa50d2e | headers_checked | hash_validated_oracle_count | 40 | NoOutPrior_A_Qualcomm_1.md5 | 1c5f20a08ab8dfa0f44d9a0ca23d174d |
| NoOutPrior_B | d06212be8b66be424237dd084a9d1401dec2ac2bf1d08543629d870c8bf01994 | headers_checked | hash_validated_oracle_count | 47 | NoOutPrior_B_Qualcomm_1.md5 | 048bca0ec216a3f88cf9fbb4bb5a61ac |
| NUT_A | d87dcae6353a680ff1c816395b578afae3ed9f1a88b56b07a24e62333e0621b7 | headers_checked | hash_validated_oracle_count | 34 | NUT_A_ericsson_5.md5 | 9dbe33badd0cc6dfcd1b53e546968fbd |
| OPFLAG_A | 1fb75cf1426c0682f324fe0dcb029561cbc50aa813b9d91aa434d061610a123a | headers_checked | hash_validated_oracle_count | 500 | OPFLAG_A_Qualcomm_1.md5 | 35eeeca175938ee262f72b7485146f94 |
| OPFLAG_B | 24cc9e1b1848673d45ecfd161a186bef7151a5e3b254ead5bcfa0ce93478cef7 | headers_checked | hash_validated_oracle_count | 98 | OPFLAG_B_Qualcomm_1.md5 | 516811eeff3ae5ee056dda290dcf2735 |
| OPFLAG_C | 17016c7abdec5fdda16fb92a6b6521adf4d9485070a00d662b44665c1e247824 | headers_checked | hash_validated_oracle_count | 96 | OPFLAG_C_Qualcomm_1.md5 | a82c4f6ee843150067bba60b9342fc50 |
| PICSIZE_A | 3e490a54e23594294fe9e12b1c01880546ab2efcc7979d87d330a8428f1a1480 | headers_checked | hash_validated_oracle_count | 10 | PICSIZE_A_Bossen_1/PICSIZE_A_Bossen_1.md5 | 910ff0e6b3739af0b4aba4f6aa6704f8 |
| PICSIZE_B | f16bbcfb92d601cc0735641c9fd248591bcded97721a3d351d89bcfb6a48c2ad | headers_checked | hash_validated_oracle_count | 10 | PICSIZE_B_Bossen_1/PICSIZE_B_Bossen_1.md5 | e5c4c20a8871aa446a344efb1755bcf9 |
| PICSIZE_C | c16e7aab1f86ce96cbeb21577f3e6f66e50eb618a1cf9203abb00fef2d71e9ad | headers_checked | hash_validated_oracle_count | 10 | PICSIZE_C_Bossen_1/PICSIZE_C_Bossen_1.md5 | 674f9f11eb956257d86782949ec00399 |
| PICSIZE_D | bfdb504691a7228cb8b0acec17c56150464eb30fc3e3df6b5bf81e4d8e1898ca | headers_checked | hash_validated_oracle_count | 10 | PICSIZE_D_Bossen_1/PICSIZE_D_Bossen_1.md5 | 5b76e0591618f5affe8b15b03d9c0bef |
| PMERGE_A | e5f1f706176e91b72d13686b8864fe1c8e8be764faa417e3cabbf0df1cf78af0 | headers_checked | hash_validated_oracle_count | 8 | PMERGE_A_TI_3/PMERGE_A_TI_3.md5 | 6e74dbd450123246c22bf01b389bbe11 |
| PMERGE_B | c4830df0041aceaada75eb7233be96a4a8d3e1fb0ee72c4f9a7f40cf5d55514b | headers_checked | hash_validated_oracle_count | 8 | PMERGE_B_TI_3/PMERGE_B_TI_3.md5 | 7d0e57ebdbbc6633c972083a17d97550 |
| PMERGE_C | ec68d2adca61ba94603fceafabcd865f17072ef67f04ec1cd97ab00ffb247944 | headers_checked | hash_validated_oracle_count | 8 | PMERGE_C_TI_3/PMERGE_C_TI_3.md5 | 7e23d874b198ec7a9f6086a20de0db76 |
| PMERGE_D | afb2952a6a4e2f446340d4053a6f5268028e3b61be52f5cee779c36275c4877a | headers_checked | hash_validated_oracle_count | 8 | PMERGE_D_TI_3/PMERGE_D_TI_3.md5 | 9771b8c8a71e41547f14032e07be7d61 |
| PMERGE_E | fc3d15683748f42229c64ce58804a9174686b79749e24f580337c2cb77e46feb | headers_checked | hash_validated_oracle_count | 8 | PMERGE_E_TI_3/PMERGE_E_TI_3.md5 | 26aed56afdb6cb7011fd5c3303379a9d |
| POC_A | 0830773b2c3f6ef85ec973f09a08dfd0f1e7a36e05936d4b1fc0b6c014a3a639 | headers_checked | hash_validated_oracle_count | 5 | POC_A_Bossen_3/POC_A_Bossen_3.md5 | faf33bec64d3e58c426ed2772903a0bf |
| PPS_A | 2382b56a09bc9bff001cc4b1e90826219c17324de944acce5dde490032322128 | headers_checked | hash_validated_oracle_count | 81 | PPS_A_qualcomm_7/PPS_A_qualcomm_7.yuv.md5 | bccf29fab6744215fe007ea3a5d6da42 |
| PS_B | 3f16134cdcaea5a2cc33a4b539693c3af1bf0e78b7c5ca051a7fb1d8ddc41ce3 | headers_checked | hash_validated_oracle_count | 25 | PS_B_VIDYO_3/PS_B_VIDYO_3_yuv.md5 | eb85521ee103333eac61a8253637eac4 |
| RAP_A | 931db64702a056cddf66dab87e80598f16031bd147ed56c25d98b3f555424029 | headers_checked | hash_validated_oracle_count | 86 | RAP_A_docomo_6.md5 | 8a536a80ed42b37b1ac5810bc046f82a |
| RAP_B | f8718a7b9a58a40845601879a167566fea4a842e4919be63569836974518224a | headers_checked | hash_validated_oracle_count | 80 | RAP_B_Bossen_2.md5 | f38befcc280f2fd0d23f48f807f843c2 |
| RPLM_A | b2994be2579c6835a16d14e8404470caa97d72d45312500703c59f3189075caa | headers_checked | hash_validated_oracle_count | 300 | RPLM_A_qualcomm_4/RPLM_A_qualcomm_4.yuv.md5 | 011794872709c341880ee42ade036ed3 |
| RPLM_B | 18d7bc82140f7f117acb48eb62c63d6dcda453bee0eda0975172b1c6012c4be2 | headers_checked | hash_validated_oracle_count | 300 | RPLM_B_qualcomm_4/RPLM_B_qualcomm_4.yuv.md5 | 653ac7c46fa7b9d7d966e3db317eb938 |
| RPS_A | e7a90335952dc5718d931adb461d90049eb558b4d08c90ff1706612f8bca4439 | headers_checked | hash_validated_oracle_count | 44 | RPS_A_docomo_5.md5 | 7f4ad6c6b3de54558b0db59629b87db9 |
| RPS_B | a5e2097201740c15824b75540b6d0d5050763681bedf7209cde80d93a3b88996 | headers_checked | hash_validated_oracle_count | 300 | RPS_B_qualcomm_5/RPS_B_qualcomm_5.yuv.md5 | 6d1ed392b067050ebd3a24a37281da03 |
| RPS_C | b0aa060407b3ad9a558a1212645a1bf137187f2dce8bf47f312efa1bd19ded6e | headers_checked | hash_validated_oracle_count | 40 | RPS_C_ericsson_5.md5 | 0306b9e3ec365115ccb7060c0a437b32 |
| RPS_D | e818e14f50bf73364b405ca3b07c407fdf191bcb2293552d3af8fc3bfcd1aca0 | headers_checked | hash_validated_oracle_count | 68 | RPS_D_ericsson_6.md5 | d0fcd71e6266b1ae1f66b9cb1802ea03 |
| RPS_E | b82c5c8c251943cc41b59e7f63f72890641fdcdbcec8e916223e4ee28a16c7ad | headers_checked | hash_validated_oracle_count | 300 | RPS_E_qualcomm_5/RPS_E_qualcomm_5.yuv.md5 | c30d38bbd7ea483dc7ffe81325118e16 |
| RPS_F | 57588b676e3d1c2b82b761671c66383fac2b5c9c3eb98644c70a8e5fed486ed6 | headers_checked | hash_validated_oracle_count | 50 | RPS_F_docomo_2.md5 | fd6e6d973d39622d197dbdf4543643f2 |
| RQT_A | c07a976943200d97ba9eb0a16a858d2c77fb3fe37410c6c6f956e5960b2f4621 | headers_checked | hash_validated_oracle_count | 2 | RQT_A_HHI_4/RQT_A_HHI_4.md5 | aa7f6ef07bf76ebddd52f7e528cd6ab8 |
| RQT_B | 49a8037bbbed4efa8de285c522d215c3c5a117dd58d17822db0ffb9808161e29 | headers_checked | hash_validated_oracle_count | 2 | RQT_B_HHI_4/RQT_B_HHI_4.md5 | a63f4f4e966e77a5eb4e65dcdcc488a4 |
| RQT_C | a7b157215595b68ddbb70d952fb786923826c26ecc448bbfe36f4df47f9abc5c | headers_checked | hash_validated_oracle_count | 2 | RQT_C_HHI_4/RQT_C_HHI_4.md5 | c82f4c88d9b2c73fabbeac8944da35a1 |
| RQT_D | 6b37468d7e4acf7d1fb97bf0062e0124785d3bda1284e1f698a1e8eb6441d76b | headers_checked | hash_validated_oracle_count | 2 | RQT_D_HHI_4/RQT_D_HHI_4.md5 | 33b58b954fa82753526fda9e9e6bc8cb |
| RQT_E | d792e442c19d9ffc45f43a8842c7a6ad57d0a51d62364c43ef97810689c2eea8 | headers_checked | hash_validated_oracle_count | 2 | RQT_E_HHI_4/RQT_E_HHI_4.md5 | 65f22431d0eb149e04c327f881ce6581 |
| RQT_F | f784a12b06a78deafc2ab5fb51af6cbaa105e1b72ebf88e2fb28536f771fac9a | headers_checked | hash_validated_oracle_count | 2 | RQT_F_HHI_4/RQT_F_HHI_4.md5 | 56ba354d919d06a33b44862dd18e9437 |
| RQT_G | 3ce2c5f477fc2762d2153a373dbc4e5a6387c98035f34dfd32008cc7667286d3 | headers_checked | hash_validated_oracle_count | 2 | RQT_G_HHI_4/RQT_G_HHI_4.md5 | 2fb67256014a6840b69ca6dad2426304 |
| SAO_A | 88f693ac4aec4dea03cb4bc0cb9d8c98a4023a015905369bb62688064fb58944 | headers_checked | hash_validated_oracle_count | 60 | SAO_A_MediaTek_4.md5 | 272e694a2262a2b34f6248f787a4431d |
| SAO_B | 77fb629a562234f2fea44886dafb597565d85433d9d9c4f22c208f5552950009 | headers_checked | hash_validated_oracle_count | 60 | SAO_B_MediaTek_5.md5 | 90cc57237e3ce47eb3b4c901e200112d |
| SAO_C | 45c4b761e6f1d833b4e206ffadea66e8f5a3f5f1984ed0c29519eb9c0d666b4d | headers_checked | hash_validated_oracle_count | 5 | SAO_C_Samsung_5/SAO_C_Samsung_5.md5 | 962290491f2f1a1e70ab7aa63d2264bc |
| SAO_D | 84e1a6307db2a05144c73a5dbc338fc286c0a4f1c45ed04fc0182f8119d80c6c | headers_checked | hash_validated_oracle_count | 5 | SAO_D_Samsung_5/SAO_D_Samsung_5.md5 | 6db9575a7737816b1d7a3a3bf3ba126c |
| SAO_E | a3a3cbf0fbf4a1335070a085d0b01aa8064889e9c74ac7081631349fbc2c5915 | headers_checked | hash_validated_oracle_count | 16 | SAO_E_Canon_4.md5 | 0b04a1378274a6ffbb8f6121e027290c |
| SAO_F | 6e663fdf83681e8989f6b5e25a4d62e0a93f1024f0c4dd785e896c6db81caeed | headers_checked | hash_validated_oracle_count | 16 | SAO_F_Canon_3.md5 | 5c295ecba132d1e68a12dd8209e86a02 |
| SAO_G | 74224b18c1864efadaf9c06427905e9cd6e847e324a100d2373e4c780726b305 | headers_checked | hash_validated_oracle_count | 16 | SAO_G_Canon_3.md5 | 1f743a3e28617b8bae3e348096edcf76 |
| SAO_H | b6a88d25b6fb67f72ec0731868d15acf2f6c3573d82ad508997e2054bb83974d | headers_checked | hash_validated_oracle_count | 4 | SAO_H_Parabola_1.md5 | 7e23c16898b14831b25d7857b69a9a9a |
| SAODBLK_A | dab095100e4cea72b8e871d01bf39bb5027b460be5c495ea3374454f4aafe507 | excluded | maintainer_exclusion | — | known-exclusions.json | Published b9c98937bcb6be445348fdc89c208f1e; oracle 23f03fffa670f7d770302c819b04f439; excluded by maintainer decision, 2026-11-04 |
| SAODBLK_B | 8f3d22ccfffe230c034f1fbcc996088282cea52e2d3e22455c395a2fbb0b91ae | excluded | maintainer_exclusion | — | known-exclusions.json | Published 031043495e9b9f4ce120afa316aa5705; oracle de53c4f4adb77faca65c994c5fa7e7d0; excluded by maintainer decision, 2026-11-04 |
| SDH_A | ff68e351105a1c9b8a0293663e0275260684b50f8be4dbdb850bd2db1c7c8e27 | headers_checked | hash_validated_oracle_count | 2 | SDH_A_Orange_4/SDH_A_Orange_4_yuv.md5 | 0a1a6a76a86dbdc7ca479e4235f5b822 |
| SLICES_A | 7440908beaa68768ee66b7af5823a28ce0716d90aa1b49de630d2c5aa555d955 | headers_checked | hash_validated_oracle_count | 9 | SLICES_A_Rovi_3.md5 | c2d63a4d145a5713afecd822032ec271 |
| SLIST_A | be5532289143b068bfe451243b750b8162017c0569046c680931987469f47095 | headers_checked | hash_validated_oracle_count | 65 | SLIST_A_Sony_5/SLIST_A_Sony_5_yuv.md5 | b00bc5362e6fad1a0e704aad8e69b863 |
| SLIST_B | d37c0a144fc67a10195b2134a1eccc268d8b37add53e91ddb74f26b690c053ec | headers_checked | hash_validated_oracle_count | 65 | SLIST_B_Sony_9/SLIST_B_Sony_9_yuv.md5 | 312b4ebb3e885587d5e12b78aa95977a |
| SLIST_C | ddbb22d8beb26a50ba547fb55c6c308264c7e09eee4a6ee9f65d1785f272709e | headers_checked | hash_validated_oracle_count | 65 | SLIST_C_Sony_4/SLIST_C_Sony_4_yuv.md5 | 61024c25cbd60e9bf86dbe3bc5b9b48b |
| SLIST_D | f3d8468f5cd0ca4e4a2e78c372500f4859f95395e9f1c9c85b357e3dddd135e8 | headers_checked | hash_validated_oracle_count | 65 | SLIST_D_Sony_9/SLIST_D_Sony_9_yuv.md5 | 8d38ce43b17627ff1f86e526bfb7ffc4 |
| SLPPLP_A | 48f9cecfe1f86654fca3eb327c0914eec475bcf0fb8f6f726632976e5b34e2a6 | headers_checked | hash_validated_oracle_count | 33 | SLPPLP_A_VIDYO_2/SLPPLP_A_VIDYO_2_yuv.md5 | 99e13916ede2bc9b77718787579104a9 |
| STRUCT_A | e511007d79d66f6988654a7f89a934cd68329be6492143cda9570e3741f5a551 | headers_checked | hash_validated_oracle_count | 60 | STRUCT_A_Samsung_7.md5 | 02f02cc9a67222004ae18ee6bebd475a |
| STRUCT_B | e58b04f1434449721957857ca38adab3865b71cb93ae9705a4361b7cb4400d8e | headers_checked | hash_validated_oracle_count | 60 | STRUCT_B_Samsung_7.md5 | 1f472e014aff120681e8482c8a04ef93 |
| TILES_A | eff78a401ecccc21d995345988f1be60ee76604cf10fa39d421c3e00668a94d6 | headers_checked | hash_validated_oracle_count | 100 | TILES_A_Cisco_2/TILES_A_Cisco_2_yuv.md5 | 946f0771fa87d49f684f96871c9c3c88 |
| TILES_B | d2f65167917e42030db804c60ca7775452445a61930ccee3fa2ad2cd3830a57a | headers_checked | hash_validated_oracle_count | 100 | TILES_B_Cisco_1/TILES_B_Cisco_1_yuv.md5 | 3382291f2b19ee2d760647d4cc756e95 |
| TMVP_A | 33556aa42355ba43575a6e6f1b579420a9217f576fc0ae07ee57fa2750a38d52 | headers_checked | archived_yuv_length | 17 | TMVP_A_MS_3.yuv | 168f8096e56b8a8c1aa03fa5fffdfd23 |
| TSCL_A | cab7d7ea801a5ee7d1ff495c98cf5e4eafa2c4fb32cc1473443f69f2457cfd16 | headers_checked | hash_validated_oracle_count | 73 | TSCL_A_VIDYO_5/TSCL_A_VIDYO_5_yuv.md5 | 27cbe23eabe1b174d745c286cc2ed8ff |
| TSCL_B | cdeff7b17f020f79f8c0d9264748f5f9104657e5900fb126858fda93be90f610 | headers_checked | hash_validated_oracle_count | 73 | TSCL_B_VIDYO_4/TSCL_B_VIDYO_4_yuv.md5 | 95e7910b85c08a01e2e9ce72c92d7d62 |
| TS_A | 8407a07198b8db84e2fb2aef5c06d2d73f32bd0e4ddf08c5a9455481dcd73141 | headers_checked | archived_yuv_length | 17 | TSKIP_A_MS_3.yuv | 4280cb848219176f4fcad51f3290210d |
| TSUNEQBD_A_MAIN10 | a0b12cc3b180a410614931c94a693852c34aa89587a934aa5af9dbaa10cd7dfc | refused | expected_refusal | — | H.265.1 scope | unequal component depths |
| TUSIZE_A | 23c9959a0dd09d9a0b9c64b980794ca18214d7a3cb0fd091ca87f98dda75acf4 | headers_checked | hash_validated_oracle_count | 5 | TUSIZE_A_Samsung_1.md5 | 5e6c198c852e92cdcca0e137fc2610c4 |
| VPSID_A | d86bb746dadb63bdf561615a1de7860b2336c7616462507453e0fcec3ddf27f3 | headers_checked | hash_validated_oracle_count | 33 | VPSID_A_VIDYO_2/VPSID_A_VIDYO_2_yuv.md5 | 99e13916ede2bc9b77718787579104a9 |
| VPSSPSPPS_A | 8501a5e5f0f191d3810a7aecab43aacac1fcc28f08cb3731158cc48a89501a27 | headers_checked | published_output_count | 6 | VPSSPSPPS_A_MainConcept_1.txt | 1ddf74263cb4953cfdfcf99c563d88ea |
| WP_A_MAIN10 | 3be359e9c70f56e478bb0c0710dcea112252541570f81856cc9f3dba3d988263 | headers_checked | hash_validated_oracle_count | 256 | WP_A_MAIN10_Toshiba_3/WP_A_MAIN10_Toshiba_3_yuv.md5 | c48e853010a234786dc87790d053b03a |
| WP_A | d052c54c39078ee6d546a431094b53df633e0c17777e2bed693e7999b0ba29c5 | headers_checked | hash_validated_oracle_count | 256 | WP_A_Toshiba_3_yuv.md5 | 127b99d0c11c930b6f248750b5848a0b |
| WP_B | c638c1c1052639ac52bf6c1ffe5b5bee55484ccfb56c13bcdc2a085fdb066799 | headers_checked | hash_validated_oracle_count | 256 | WP_B_Toshiba_3_yuv.md5 | 82d39f8cd09e532b8fa9d35894c96258 |
| WP_B_MAIN10 | 0b307c3ae71258ec5f322cb85cda15ca766b93ed9a01fd3d0411ac8d113165fd | headers_checked | hash_validated_oracle_count | 256 | WP_MAIN10_B_Toshiba_3_yuv.md5 | ed891c60a17f5cbc64e1848f0ea9cf45 |
| WPP_A_MAIN10 | 3b0d7323bf81d64d3c1443c9cebb8df6a42e0719ed422e64125e03971999b956 | headers_checked | archived_yuv_length | 48 | WPP_A_ericsson_MAIN10_2.yuv | e68b9d75ff9cf1d9486e964051351f69 |
| WPP_A | 54d896d9fbdfa0aae15629001105c6ee132c8459e152abb06efc62cead4324ae | headers_checked | archived_yuv_length | 48 | WPP_A_ericsson_MAIN_2.yuv | cd7e815eb47e8138fec2185d4de84304 |
| WPP_B_MAIN10 | 64de8e2d12825ad760012421f6c334c71d62bf7a6d5c35af96eb76548f911fbc | headers_checked | archived_yuv_length | 48 | WPP_B_ericsson_MAIN10_2.yuv | d8290fc1eec3a196aa796c92e71a779b |
| WPP_B | f1d18f737a9380f6ce58b6aeae158af458c5b557e06dd24977b7a5c6e095b9b2 | headers_checked | archived_yuv_length | 48 | WPP_B_ericsson_MAIN_2.yuv | e37c7e561a1226640a7bf98e81df78b1 |
| WPP_C_MAIN10 | 707d0aa04dbb25f57be710c3fa2216d93c0a4f4d0cc950b2564f2aab6686d047 | headers_checked | archived_yuv_length | 48 | WPP_C_ericsson_MAIN10_2.yuv | aadac4d468d5950551d606c1016abc9c |
| WPP_C | 21cf0a7c5f6fba5a76c7132a2714313d120d1443a0130400fb55ab1e454d5bda | headers_checked | archived_yuv_length | 48 | WPP_C_ericsson_MAIN_2.yuv | e067aa3a6a12cd5743849ded793c8d3f |
| WPP_D_MAIN10 | eb411ab3baedf154ad85db24f11e4537177f04e995df4d609ef6f4ae9641aaa9 | headers_checked | archived_yuv_length | 48 | WPP_D_ericsson_MAIN10_2.yuv | dde9aa65b78a8ca5602d834f234655d4 |
| WPP_D | 30eec63f2324aa982fb91bd4c1c551c833c253ba711ee131aa9cc4d322398caf | headers_checked | archived_yuv_length | 48 | WPP_D_ericsson_MAIN_2.yuv | f710612103f386c415be3e6300693451 |
| WPP_E_MAIN10 | f22251fff614135de5a5b2ada01f793ea4c511fad773acda0e8fd411dfe84e76 | headers_checked | archived_yuv_length | 48 | WPP_E_ericsson_MAIN10_2.yuv | f12e746306c59544d6162de0b62c4649 |
| WPP_E | c8fe49762a13e1cc2b033308bb22aeb35da0c96f94ca8ef7d87d95bdadaeaa2c | headers_checked | archived_yuv_length | 48 | WPP_E_ericsson_MAIN_2.yuv | 485798dbf95ad61232075df2f294aa3f |
| WPP_F_MAIN10 | f772dc0d6ec106478ced3ac598e459d9273e260fb56734e055364653f46e79f1 | headers_checked | archived_yuv_length | 48 | WPP_F_ericsson_MAIN10_2.yuv | 210055a33d7d912fda6a3c40764b0482 |
| WPP_F | e8566e0e48509592dfaf7d314b7e292f51cede045559e3c32b447a9822a8b949 | headers_checked | archived_yuv_length | 48 | WPP_F_ericsson_MAIN_2.yuv | 2aaf16274fe8e799d72fa08a4963850d |
| SYNTHETIC_main10-ctu16-tu4 | 2fe86d52ab736e4aea1baa5af766af033fdd71cbe16a17e9e66ddc48415a1ab7 | headers_checked | synthetic_registered_recipe | 2 | bench/hevc-conformance/synthetic.json#main10-ctu16-tu4 | b1375b33e1302e52c47bfd3875dedeec (synthetic whole output) |
| SYNTHETIC_main10-ctu32-tu16 | fb810c44931ead2b00c9a21e8e71b4f65b21239c5f49c30924b233aab018c6bc | headers_checked | synthetic_registered_recipe | 2 | bench/hevc-conformance/synthetic.json#main10-ctu32-tu16 | 0f1ccadf4242882284897fa0b15def6b (synthetic whole output) |
| SYNTHETIC_main10-bypass | 1e3fba4822359e126953b5d409dd768af2560f939ae22dc9caae30ffceecc375 | headers_checked | synthetic_registered_recipe | 2 | bench/hevc-conformance/synthetic.json#main10-bypass | 1c1eda47420221f7c6896a45cf5effa2 (synthetic whole output) |
| SYNTHETIC_main10-bypass-changed | 34f99b0a9e7f1e79e394f412d2b7705c1667b95a7e798c5a62d6e5aaff14cc06 | headers_checked | synthetic_registered_recipe | 2 | bench/hevc-conformance/synthetic.json#main10-bypass-changed | 26200dfbe752f8254e37268a6cb40e5b (synthetic whole output) |
