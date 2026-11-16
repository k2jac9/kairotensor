Batch and bulk both use atomic edits. Batch seeds and registers one item at a time. Bulk uses the bulk setup APIs. Scan uses the full-scan baseline. Setup includes opening the database, seeding evidence, and registering rules. Phase durations and database sizes are medians.

| Scenario | Rules | Edits | Arm | Seed ms | Register ms | Setup ms | Edit ms | Total ms | Database KiB |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| dense | 128 | 1 | batch | 103.423 | 162.210 | 271.289 | 4.208 | 275.575 | 140.000 |
| dense | 128 | 1 | bulk | 1.496 | 13.786 | 16.777 | 4.282 | 21.112 | 140.000 |
| dense | 128 | 1 | scan | 0.774 | 1.220 | 3.007 | 0.806 | 3.824 | 32.000 |
| dense | 128 | 32 | batch | 137.725 | 228.259 | 357.699 | 152.202 | 591.520 | 280.000 |
| dense | 128 | 32 | bulk | 1.592 | 13.992 | 17.050 | 182.391 | 200.160 | 280.000 |
| dense | 128 | 32 | scan | 0.859 | 1.259 | 3.313 | 33.962 | 42.969 | 56.000 |
| sparse | 128 | 1 | batch | 145.392 | 185.158 | 355.913 | 1.087 | 357.211 | 132.000 |
| sparse | 128 | 1 | bulk | 1.485 | 13.938 | 16.768 | 1.137 | 17.976 | 132.000 |
| sparse | 128 | 1 | scan | 0.796 | 1.303 | 3.297 | 0.835 | 4.144 | 32.000 |
| sparse | 128 | 32 | batch | 109.430 | 180.574 | 294.625 | 39.122 | 330.687 | 140.000 |
| sparse | 128 | 32 | bulk | 1.546 | 14.002 | 17.136 | 36.684 | 54.175 | 140.000 |
| sparse | 128 | 32 | scan | 0.790 | 1.258 | 3.150 | 25.212 | 28.747 | 32.000 |
| sparse | 1024 | 128 | batch | 1164.432 | 2565.256 | 3690.273 | 180.397 | 3870.816 | 708.000 |
| sparse | 1024 | 128 | bulk | 6.341 | 206.607 | 214.389 | 279.073 | 493.250 | 708.000 |
| sparse | 1024 | 128 | scan | 1.052 | 4.720 | 6.788 | 123.107 | 129.927 | 136.000 |

Speed ratios are measured within each matched trial and reported as median [min, max]. Values above 1 favor bulk. A zero measured setup time makes the setup ratio unavailable.

| Scenario | Rules | Edits | Triplets | Batch / bulk setup | Batch / bulk total | Scan / bulk total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| dense | 128 | 1 | 6 | 16.195 [15.559, 22.877] | 12.607 [4.555, 13.174] | 0.183 [0.055, 0.336] |
| dense | 128 | 32 | 6 | 19.929 [7.603, 33.118] | 2.557 [1.767, 5.040] | 0.193 [0.140, 0.489] |
| sparse | 128 | 1 | 6 | 20.851 [16.816, 24.449] | 19.485 [15.801, 22.917] | 0.228 [0.213, 0.331] |
| sparse | 128 | 32 | 6 | 17.328 [16.089, 24.651] | 6.183 [5.045, 9.792] | 0.551 [0.435, 1.609] |
| sparse | 1024 | 128 | 6 | 16.617 [14.886, 34.549] | 9.298 [6.880, 14.753] | 0.258 [0.226, 0.354] |
