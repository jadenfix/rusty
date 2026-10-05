import unittest

from shop.discounts import apply_discount


class DiscountTest(unittest.TestCase):
    def test_no_code(self):
        self.assertEqual(apply_discount(1000, None), 1000)

    def test_tenoff(self):
        self.assertEqual(apply_discount(1000, "tenoff"), 900)

    def test_bulk_applies_at_the_minimum(self):
        self.assertEqual(apply_discount(5000, "BULK"), 4250)

    def test_bulk_takes_fifteen_percent_off(self):
        self.assertEqual(apply_discount(8000, "BULK"), 6800)

    def test_bulk_ignored_below_minimum(self):
        self.assertEqual(apply_discount(4999, "BULK"), 4999)

    def test_unknown_code(self):
        with self.assertRaises(ValueError):
            apply_discount(1000, "FREE")


if __name__ == "__main__":
    unittest.main()
