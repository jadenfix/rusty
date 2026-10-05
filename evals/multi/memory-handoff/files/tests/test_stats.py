import json, unittest
from stats import mean

class StatsTest(unittest.TestCase):
    def test_mean_of_fixture(self):
        data = json.load(open("fixtures/data.json"))["values"]
        self.assertEqual(mean(data), 2)

if __name__ == "__main__":
    unittest.main()
