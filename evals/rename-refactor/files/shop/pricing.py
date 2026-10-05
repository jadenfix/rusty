def calc_total(lines, tax_rate=0.0):
    subtotal = sum(qty * price for qty, price in lines)
    return round(subtotal * (1 + tax_rate), 2)
