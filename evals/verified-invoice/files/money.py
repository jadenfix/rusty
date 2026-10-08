from decimal import Decimal

def cents(value):
    return int(Decimal(value) * 100)
