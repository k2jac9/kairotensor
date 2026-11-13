"""Illustrative exact oracle only; not a production belief or proof engine.

Factors are declared joint likelihoods for canonical dependence groups.
Duplicate reports of one observation must retain the same group identity.
Distinct identifiers cannot establish conditional independence: the caller
must supply that modeling assumption explicitly. Proofs below are already
grounded leaf certificates; cyclic rule discovery is outside this oracle.
"""

from dataclasses import dataclass
from fractions import Fraction as F
import unittest


@dataclass(frozen=True)
class Factor:
    group: str
    likelihood: tuple[F, ...]


class ImpossibleEvidence(ValueError):
    """The declared model assigns zero probability to active evidence."""


def logical_status(active, supports=(), refutations=()):
    """Evaluate OR-of-AND leaf certificates independently on both sides."""
    if any(not proof for proof in (*supports, *refutations)):
        raise ValueError("Represent axioms as explicit versioned leaves")
    positive = any(set(proof) <= set(active) for proof in supports)
    negative = any(set(proof) <= set(active) for proof in refutations)
    return ("neither", "support_only", "refutation_only", "both")[
        int(positive) + 2 * int(negative)
    ]


def posterior(prior, factors=(), *, conditional_independence=False):
    """Rebuild from prior and active manifest; never divide an old posterior."""
    prior, factors = tuple(map(F, prior)), tuple(factors)
    if not prior or sum(prior, F(0)) != 1 or any(p < 0 for p in prior):
        raise ValueError("Prior must be a finite probability distribution")
    groups = [factor.group for factor in factors]
    if any(not group for group in groups) or len(groups) != len(set(groups)):
        raise ValueError("Each dependence group needs exactly one joint factor")
    if len(factors) > 1 and not conditional_independence:
        raise ValueError("Declare group independence conditional on the state")
    weights = list(prior)
    for factor in factors:
        likelihood = tuple(map(F, factor.likelihood))
        if len(likelihood) != len(prior) or any(x < 0 or x > 1 for x in likelihood):
            raise ValueError("Likelihood must match states and lie in [0, 1]")
        weights = [weight * value for weight, value in zip(weights, likelihood)]
    normalizer = sum(weights, F(0))
    if normalizer == 0:
        raise ImpossibleEvidence("Zero normalizer; no posterior exists")
    return tuple(weight / normalizer for weight in weights)


def total_variation(p, q):
    p, q = posterior(p), posterior(q)
    if len(p) != len(q):
        raise ValueError("Distributions must use the same ordered state space")
    return sum((abs(x - y) for x, y in zip(p, q)), F(0)) / 2


def expected_utility(p, utility):
    if len(p) != len(utility) or any(value < 0 or value > 1 for value in utility):
        raise ValueError("Utilities must match states and lie in [0, 1]")
    return sum((prob * value for prob, value in zip(p, utility)), F(0))


def best_action(p, actions):
    """A deterministic first-index tie break makes the tight example exact."""
    if not actions:
        raise ValueError("Action set must be nonempty")
    return max(range(len(actions)), key=lambda index: expected_utility(p, actions[index]))


class BeliefOracleTests(unittest.TestCase):
    def setUp(self):
        self.prior = (F(1, 2), F(1, 2))

    def test_independent_four_statuses(self):
        supports, refutations = (("a", "b"), ("c",)), (("d",),)
        for active, status in [
            ((), "neither"), (("a", "b"), "support_only"),
            (("d",), "refutation_only"), (("c", "d"), "both"),
        ]:
            self.assertEqual(logical_status(active, supports, refutations), status)
        self.assertEqual(logical_status(("a",), supports, refutations), "neither")
        self.assertEqual(logical_status(("a", "c"), supports, refutations), "support_only")

    def test_zero_likelihood_retraction_restores_prior(self):
        factor = Factor("observation-1", (F(0), F(1)))
        self.assertEqual(posterior(self.prior, (factor,)), (F(0), F(1)))
        self.assertEqual(posterior(self.prior, ()), self.prior)

    def test_impossible_evidence_is_explicit(self):
        impossible = Factor("impossible", (F(0), F(0)))
        with self.assertRaises(ImpossibleEvidence):
            posterior(self.prior, (impossible,))
        with self.assertRaises(ImpossibleEvidence):
            posterior((F(1), F(0)), (Factor("excluded", (F(0), F(1))),))

    def test_copies_cannot_be_second_factor(self):
        report = Factor("shared-origin", (F(1, 4), F(3, 4)))
        self.assertEqual(posterior(self.prior, (report,)), (F(1, 4), F(3, 4)))
        with self.assertRaises(ValueError):
            posterior(self.prior, (report, report), conditional_independence=True)

    def test_independence_is_an_assumption(self):
        first = Factor("observation-1", (F(1, 4), F(3, 4)))
        second = Factor("observation-2", (F(1, 4), F(3, 4)))
        with self.assertRaises(ValueError):
            posterior(self.prior, (first, second))
        self.assertEqual(
            posterior(self.prior, (first, second), conditional_independence=True),
            (F(1, 10), F(9, 10)),
        )

    def test_one_step_regret_bound_is_tight(self):
        epsilon = F(1, 4)
        p, q = (F(3, 4), F(1, 4)), self.prior
        actions = ((F(0), F(1)), (F(1), F(0)))
        exact, compressed = best_action(p, actions), best_action(q, actions)
        regret = expected_utility(p, actions[exact]) - expected_utility(p, actions[compressed])
        self.assertEqual(total_variation(p, q), epsilon)
        self.assertEqual(regret, 2 * epsilon)

    def test_conditioning_can_amplify_compression_error(self):
        epsilon = F(1, 10)
        p, q = (1 - epsilon, epsilon, F(0)), (1 - epsilon, F(0), epsilon)
        event = Factor("rare-event", (F(0), F(1), F(1)))
        self.assertEqual(total_variation(p, q), epsilon)
        self.assertEqual(total_variation(posterior(p, (event,)), posterior(q, (event,))), 1)

    def test_unversioned_empty_certificate_is_rejected(self):
        with self.assertRaises(ValueError):
            logical_status((), ((),), ())


if __name__ == "__main__":
    unittest.main(verbosity=2)
