import unittest

from shop.cart import Cart


class CartTest(unittest.TestCase):
    def test_total_with_tax(self):
        cart = Cart()
        cart.add("tea", 2)
        self.assertEqual(cart.total(), 974)


if __name__ == "__main__":
    unittest.main()
