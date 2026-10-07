def total(lines, tax):
    return round(sum(quantity * float(price) * 100 for quantity, price in lines) * (1 + float(tax)))
