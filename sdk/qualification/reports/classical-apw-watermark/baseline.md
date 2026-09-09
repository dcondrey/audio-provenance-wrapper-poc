# Classical Watermark qualification baseline

- Production implementation: `apw-watermark-lepqim-v1`
- Experimental implementations included: none
- Qualification complete: `true`
- Meets every predeclared target: `false`
- False positives: 0 / 37000 (rate 0.00000000, 95% upper bound 0.00300300)
- Exact mark recovery: 512 / 740 (rate 0.69189189)
- Recording association recovery: 0.98168498
- Material-alteration rejection: 0.95512821
- Association predicates: 0.50 s local regions, 0.75 s maximum unexplained gap, 0.80 minimum coverage
- Dataset coverage: 1000 null; 13 recovery real; 13 association; 13 adversarial items

`apw-watermark-neural` is not linked into the qualification runner. Mark recovery is a locator measurement, not authentication; signed-record association and signature verification remain separate gates.
