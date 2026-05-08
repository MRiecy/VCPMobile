import sys
content = open(r'G:\\VCPMobile\\.tmp_plan_content.txt', 'r', encoding='utf-8').read()
with open(r'C:\Users\32595\.kimi\plans\bobbi-morse-jubilee-maria-hill.md', 'w', encoding='utf-8') as f:
    f.write(content)
print('OK')
